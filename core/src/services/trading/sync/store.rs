use anyhow::{ensure, Context};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use tokio_postgres::{Client, Row, Transaction};

use super::*;
use crate::{database::{db_client, models::auth::UserId, queries::trading212_import}, services::trading::provider};

pub(super) fn now() -> DateTime<Utc> {DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).expect("current UTC timestamp")}
pub(super) fn epoch() -> DateTime<Utc> {DateTime::from_timestamp(0,0).expect("Unix epoch")}
fn unavailable() -> anyhow::Error {T212SyncError{code:"TradingSyncUnavailable",message:"A verified Trading 212 account and configured server encryption key are required."}.into()}

pub(crate) fn run_from_row(r:&Row)->T212SyncRun {
    T212SyncRun{id:r.get("id"),account_id:r.get("account_id"),trigger:r.get("trigger"),status:r.get("status"),phase:r.get("phase"),range_from:r.get("range_from"),range_to:r.get("range_to"),cursor:r.get("cursor"),report_from:r.get("report_from"),report_to:r.get("report_to"),report_id:r.get("report_id"),next_attempt_at:r.get("next_attempt_at"),created_at:r.get("created_at"),started_at:r.get("started_at"),finished_at:r.get("finished_at"),error_code:r.get("error_code"),error_message:r.get("error_message"),inserted_count:r.get("inserted_count"),matched_count:r.get("matched_count"),review_count:r.get("review_count"),skipped_count:r.get("skipped_count")}
}
pub(crate) fn batch_from_row(r:&Row)->anyhow::Result<T212ImportBatchSummary> {
    Ok(T212ImportBatchSummary{id:r.get("id"),account_id:r.get("account_id"),sync_run_id:r.get("sync_run_id"),statement_job_id:r.get("statement_job_id"),source:r.get("source"),status:r.get("status"),report_from:r.get("report_from"),report_to:r.get("report_to"),created_at:r.get("created_at"),applied_at:r.get("applied_at"),inserted_count:r.get("inserted_count"),matched_count:r.get("matched_count"),review_count:r.get("review_count"),skipped_count:r.get("skipped_count"),skipped_actions:serde_json::from_value(r.get("skipped_actions"))?})
}

/// A management read is SQL-only, including unavailable/key-disabled states.
pub(crate) async fn status(user_id:&UserId,db:&Client)->anyhow::Result<T212SyncStatus> {
    let connection=db.query_opt("SELECT environment,credentials IS NOT NULL AS configured,account FROM trading_connection WHERE user_id=$1",&[&user_id.as_str()]).await?;
    let accounts=db.query("SELECT * FROM t212_sync_account WHERE user_id=$1 ORDER BY broker_account_id,id",&[&user_id.as_str()]).await?.into_iter().map(|r|T212SyncAccount{id:r.get("id"),environment:r.get("environment"),broker_account_id:r.get("broker_account_id"),currency:r.get("currency"),schedule_enabled:r.get("schedule_enabled"),next_run_at:r.get("next_run_at"),backfill_cursor:r.get("backfill_cursor"),backfill_complete:r.get("backfill_complete"),synced_through:r.get("synced_through"),last_success_at:r.get("last_success_at")}).collect::<Vec<_>>();
    let broker_id=connection.as_ref().and_then(|r|r.get::<_,Option<Value>>("account")).and_then(|v|v.get("id").and_then(Value::as_str).map(str::to_owned));
    let environment=connection.as_ref().and_then(|r|r.get::<_,Option<String>>("environment"));
    let configured=connection.as_ref().is_some_and(|r|r.get::<_,bool>("configured") && matches!(environment.as_deref(),Some("demo"|"live")));
    let current_account_id=if configured {accounts.iter().find(|a|Some(&a.broker_account_id)==broker_id.as_ref() && Some(a.environment.as_str())==environment.as_deref()).map(|a|a.id.clone())} else {None};
    let unavailable_reason=if !configured {Some("Connect a Trading 212 account to import its activity.".into())}
        else if current_account_id.is_none() {Some("Reconnect Trading 212 to verify its stable account identity.".into())}
        else if provider::check_master_key().is_err() {Some("The server Trading 212 encryption key is unavailable. Existing staged activity can still be reviewed.".into())} else {None};
    let runs=db.query("SELECT * FROM t212_sync_run WHERE user_id=$1 AND (status IN ('queued','running','waiting','needs_review') OR id IN (SELECT id FROM t212_sync_run WHERE user_id=$1 ORDER BY created_at DESC,id DESC LIMIT 20)) ORDER BY created_at DESC,id DESC",&[&user_id.as_str()]).await?.iter().map(run_from_row).collect();
    let review_batches=db.query("SELECT * FROM t212_import_batch WHERE user_id=$1 AND status='needs_review' ORDER BY created_at,id",&[&user_id.as_str()]).await?.iter().map(batch_from_row).collect::<anyhow::Result<Vec<_>>>()?;
    Ok(T212SyncStatus{available:unavailable_reason.is_none(),unavailable_reason,current_account_id,accounts,runs,review_batches})
}

pub(super) async fn current_connection(user_id:&UserId,tx:&Transaction<'_>)->anyhow::Result<Row> {
    trading212_import::lock_owner(user_id,tx).await?;
    provider::check_master_key().map_err(|_|unavailable())?;
    tx.query_opt("SELECT c.*,a.id AS sync_account_id FROM trading_connection c JOIN t212_sync_account a ON a.user_id=c.user_id AND a.broker_account_id=c.account->>'id' AND a.environment=c.environment WHERE c.user_id=$1 AND c.environment IN ('demo','live') AND c.credentials IS NOT NULL FOR UPDATE OF c",&[&user_id.as_str()]).await?.ok_or_else(unavailable)
}

pub(crate) async fn save_schedule(user_id:&UserId,enabled:bool)->anyhow::Result<T212SyncStatus> {
    let mut db=db_client().await?;let tx=db.transaction().await?;
    let connection=current_connection(user_id,&tx).await?;let account:String=connection.get("sync_account_id");
    tx.execute("UPDATE t212_sync_account SET next_run_at=CASE WHEN NOT $3 THEN NULL WHEN NOT schedule_enabled THEN clock_timestamp() ELSE next_run_at END,schedule_enabled=$3 WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&account,&enabled]).await?;
    tx.commit().await?;status(user_id,&db).await
}

async fn enqueue_on(user_id:&UserId,tx:&Transaction<'_>,account:&Row,generation:&str,trigger:&str,at:DateTime<Utc>)->anyhow::Result<Row> {
    let account_id:String=account.get("id");
    if let Some(active)=tx.query_opt("SELECT * FROM t212_sync_run WHERE user_id=$1 AND account_id=$2 AND status IN ('queued','running','waiting','needs_review') FOR UPDATE",&[&user_id.as_str(),&account_id]).await? {
        if trigger!="scheduled" {
            ensure!(active.get::<_,String>("status")!="needs_review",T212SyncError{code:"TradingSyncReviewRequired",message:"Resolve pending account matches before starting another report."});
            ensure!(trigger!="full_history",T212SyncError{code:"TradingSyncAlreadyRunning",message:"A sync is already active. Finish it before rescanning all history."});
        }
        return Ok(active);
    }
    let complete:bool=account.get("backfill_complete");
    let from=if trigger=="full_history" {epoch()} else if !complete {account.get("backfill_cursor")} else {account.get::<_,Option<DateTime<Utc>>>("synced_through").unwrap_or(epoch()).checked_sub_signed(TimeDelta::days(7)).unwrap_or(epoch()).max(epoch())};
    let from=from.min(at);
    let id=uuid::Uuid::new_v4().to_string();
    Ok(tx.query_one("INSERT INTO t212_sync_run(user_id,id,account_id,connection_generation,trigger,range_from,range_to,cursor,created_at,next_attempt_at) VALUES($1,$2,$3,$4,$5,$6,$7,$6,$7,$7) RETURNING *",&[&user_id.as_str(),&id,&account_id,&generation,&trigger,&from,&at]).await?)
}

pub(crate) async fn request_run(user_id:&UserId,full_history:bool)->anyhow::Result<T212SyncRunResponse> {
    let mut db=db_client().await?;let tx=db.transaction().await?;let connection=current_connection(user_id,&tx).await?;
    let account=tx.query_one("SELECT * FROM t212_sync_account WHERE user_id=$1 AND id=$2 FOR UPDATE",&[&user_id.as_str(),&connection.get::<_,String>("sync_account_id")]).await?;
    let run=enqueue_on(user_id,&tx,&account,&connection.get::<_,String>("generation"),if full_history {"full_history"} else {"sync_now"},now()).await?;
    tx.commit().await?;Ok(T212SyncRunResponse{run:run_from_row(&run),status:status(user_id,&db).await?})
}

pub(super) async fn schedule_due(db:&mut Client,at:DateTime<Utc>)->anyhow::Result<()> {
    let due=db.query("SELECT a.user_id,a.id FROM t212_sync_account a JOIN auth_users u ON u.id=a.user_id WHERE a.schedule_enabled AND a.next_run_at<=$1 AND u.status='active' AND u.username IS NOT NULL AND u.password_hash IS NOT NULL AND u.totp_secret_encrypted IS NOT NULL ORDER BY a.next_run_at,a.user_id,a.id",&[&at]).await?;
    for row in due {
        let user=UserId::from_database(row.get("user_id"));let account_id:String=row.get("id");
        let tx=db.transaction().await?;
        let connection=match current_connection(&user,&tx).await {Ok(c)=>c,Err(_)=>continue};
        if connection.get::<_,String>("sync_account_id")!=account_id {continue;}
        let account=tx.query_one("SELECT * FROM t212_sync_account WHERE user_id=$1 AND id=$2 FOR UPDATE",&[&user.as_str(),&account_id]).await?;
        if !account.get::<_,bool>("schedule_enabled") || account.get::<_,Option<DateTime<Utc>>>("next_run_at").is_none_or(|d|d>at) {continue;}
        enqueue_on(&user,&tx,&account,&connection.get::<_,String>("generation"),"scheduled",at).await?;
        tx.execute("UPDATE t212_sync_account SET next_run_at=$3::timestamptz+interval '24 hours' WHERE user_id=$1 AND id=$2",&[&user.as_str(),&account_id,&at]).await?;
        tx.commit().await?;
    }
    Ok(())
}

pub(super) async fn due_steps(db:&Client)->anyhow::Result<Vec<(UserId,String)>> {
    Ok(db.query("SELECT r.user_id,r.account_id FROM t212_sync_run r JOIN auth_users u ON u.id=r.user_id WHERE r.status IN ('queued','running','waiting') AND r.next_attempt_at<=clock_timestamp() AND u.status='active' AND u.username IS NOT NULL AND u.password_hash IS NOT NULL AND u.totp_secret_encrypted IS NOT NULL ORDER BY r.next_attempt_at,r.created_at LIMIT 32",&[]).await?.into_iter().map(|r|(UserId::from_database(r.get(0)),r.get(1))).collect())
}

pub(super) struct Step {
    pub run:T212SyncRun,
    pub generation:String,
    pub environment:String,
    pub credentials:String,
    pub account:Value,
    pub requested_at:Option<DateTime<Utc>>,
}

pub(super) async fn claim_step(user_id:&UserId,account_id:&str,db:&mut Client)->anyhow::Result<Option<Step>> {
    if !db.query_one("SELECT pg_try_advisory_lock(hashtextextended('doughbox:t212-sync:'||$1::text||':'||$2::text,0))",&[&user_id.as_str(),&account_id]).await?.get::<_,bool>(0){return Ok(None);}
    let tx=db.transaction().await?;let connection=current_connection(user_id,&tx).await?;
    ensure!(connection.get::<_,String>("sync_account_id")==account_id,"Trading 212 sync account changed");
    let Some(mut run)=tx.query_opt("SELECT * FROM t212_sync_run WHERE user_id=$1 AND account_id=$2 AND status IN ('queued','running','waiting') AND next_attempt_at<=clock_timestamp() FOR UPDATE",&[&user_id.as_str(),&account_id]).await? else {tx.commit().await?;return Ok(None)};
    let generation:String=connection.get("generation");
    ensure!(run.get::<_,String>("connection_generation")==generation,"Trading 212 sync generation changed");
    if run.get::<_,DateTime<Utc>>("cursor")>=run.get::<_,DateTime<Utc>>("range_to") {
        tx.execute("UPDATE t212_sync_run SET status='succeeded',finished_at=clock_timestamp() WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&run.get::<_,String>("id")]).await?;
        tx.commit().await?;return Ok(None);
    }
    if run.get::<_,Option<DateTime<Utc>>>("report_from").is_none() {
        let from:DateTime<Utc>=run.get("cursor");
        let mut pending:Vec<(DateTime<Utc>,DateTime<Utc>)>=serde_json::from_value(run.get("pending_intervals"))?;
        let to=if !pending.is_empty(){let (left,right)=pending.remove(0);ensure!(left==from,"Sync split coverage is not consecutive");right}
            else {(from+TimeDelta::days(365)).min(run.get("range_to"))};
        run=tx.query_one("UPDATE t212_sync_run SET report_from=$3,report_to=$4,pending_intervals=$5,phase='request_report',report_id=NULL,report_requested_at=NULL WHERE user_id=$1 AND id=$2 RETURNING *",&[&user_id.as_str(),&run.get::<_,String>("id"),&from,&to,&serde_json::to_value(pending)?]).await?;
    }
    let requested_at=run.get("report_requested_at");
    let run=tx.query_one("UPDATE t212_sync_run SET status='running',started_at=COALESCE(started_at,clock_timestamp()),error_code=NULL,error_message=NULL WHERE user_id=$1 AND id=$2 RETURNING *",&[&user_id.as_str(),&run.get::<_,String>("id")]).await?;
    tx.commit().await?;
    Ok(Some(Step{run:run_from_row(&run),generation,environment:connection.get("environment"),credentials:connection.get("credentials"),account:connection.get("account"),requested_at}))
}

/// Metadata writes share the worker's lock-owning client and current-generation
/// fence. No statement/financial operation can continue on a replacement session.
pub(super) async fn step_transaction<'a>(user_id:&UserId,db:&'a mut Client,step:&Step)->anyhow::Result<Transaction<'a>> {
    let tx=db.transaction().await?;
    trading212_import::lock_owner(user_id,&tx).await?;
    trading212_import::validate_context(user_id,&tx,&T212ImportContext{source:T212ImportSource::Trading212Api,account_id:Some(step.run.account_id.clone()),sync_run_id:Some(step.run.id.clone()),statement_job_id:None}).await?;
    let row=tx.query_one("SELECT connection_generation FROM t212_sync_run WHERE user_id=$1 AND id=$2 FOR UPDATE",&[&user_id.as_str(),&step.run.id]).await?;
    ensure!(row.get::<_,String>(0)==step.generation,"Trading 212 sync generation changed");
    Ok(tx)
}

pub(super) async fn defer(user_id:&UserId,db:&mut Client,step:&Step,seconds:u64,code:&str,message:&str)->anyhow::Result<()> {
    let tx=step_transaction(user_id,db,step).await?;
    let delay=i64::try_from(seconds).ok().and_then(TimeDelta::try_seconds).and_then(|d|now().checked_add_signed(d)).unwrap_or(DateTime::<Utc>::MAX_UTC);
    tx.execute("UPDATE t212_sync_run SET status='waiting',next_attempt_at=$3,error_code=$4,error_message=$5 WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&delay,&code,&message]).await?;
    tx.commit().await?;Ok(())
}

pub(super) async fn fail(user_id:&UserId,db:&mut Client,step:&Step,code:&str,message:&str)->anyhow::Result<()> {
    let tx=step_transaction(user_id,db,step).await?;
    tx.execute("UPDATE t212_sync_run SET status='failed',finished_at=clock_timestamp(),error_code=$3,error_message=$4 WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&code,&message]).await?;
    tx.commit().await?;Ok(())
}

pub(super) async fn switch_to_history(user_id:&UserId,db:&mut Client,step:&Step)->anyhow::Result<()> {
    let tx=step_transaction(user_id,db,step).await?;
    tx.execute("UPDATE t212_sync_run SET phase='history_orders',status='queued',report_id=NULL,report_to=range_to,pending_intervals='[]',report_requested_at=NULL,error_code=NULL,error_message=NULL,next_attempt_at=clock_timestamp() WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id]).await?;
    tx.commit().await?;Ok(())
}

pub(super) async fn save_history_cursor(user_id:&UserId,db:&mut Client,step:&Step,phase:&str,report_id:Option<&str>)->anyhow::Result<()> {
    let tx=step_transaction(user_id,db,step).await?;
    tx.execute("UPDATE t212_sync_run SET phase=$3,report_id=$4,next_attempt_at=clock_timestamp(),error_code=NULL,error_message=NULL,status=CASE WHEN status='needs_review' THEN status ELSE 'queued' END WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&phase,&report_id]).await?;
    tx.commit().await?;Ok(())
}

pub(super) async fn complete_history(user_id:&UserId,db:&mut Client,step:&Step)->anyhow::Result<()> {
    let tx=step_transaction(user_id,db,step).await?;
    let pending:i64=tx.query_one("SELECT count(*) FROM t212_import_batch WHERE user_id=$1 AND sync_run_id=$2 AND status='needs_review'",&[&user_id.as_str(),&step.run.id]).await?.get(0);
    if pending>0 {tx.commit().await?;return Ok(());}
    let to=step.run.report_to.context("Missing report interval")?;
    let complete=to>=step.run.range_to;
    tx.execute("UPDATE t212_sync_run SET cursor=$3,status=CASE WHEN $4 THEN 'succeeded' ELSE 'queued' END,phase='request_report',report_id=NULL,report_from=NULL,report_to=NULL,report_requested_at=NULL,next_attempt_at=clock_timestamp(),finished_at=CASE WHEN $4 THEN clock_timestamp() ELSE NULL END,error_code=NULL,error_message=NULL WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&to,&complete]).await?;
    tx.execute("UPDATE t212_sync_account SET backfill_cursor=GREATEST(backfill_cursor,$3),backfill_complete=backfill_complete OR $4,synced_through=CASE WHEN $4 THEN GREATEST(synced_through,$3) ELSE synced_through END,last_success_at=CASE WHEN $4 THEN clock_timestamp() ELSE last_success_at END WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.account_id,&to,&complete]).await?;
    tx.commit().await?;Ok(())
}

pub(super) async fn split(user_id:&UserId,db:&mut Client,step:&Step,min_seconds:i64)->anyhow::Result<bool> {
    let from=step.run.report_from.context("Missing sync interval")?;let to=step.run.report_to.context("Missing sync interval")?;
    if to.signed_duration_since(from)<=TimeDelta::seconds(min_seconds){return Ok(false);}
    let middle=DateTime::from_timestamp(from.timestamp()+(to.timestamp()-from.timestamp())/2,0).context("Invalid report split")?;
    if middle<=from || middle>=to {return Ok(false);}
    let tx=step_transaction(user_id,db,step).await?;
    let row=tx.query_one("SELECT pending_intervals FROM t212_sync_run WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id]).await?;
    let mut pending:Vec<(DateTime<Utc>,DateTime<Utc>)>=serde_json::from_value(row.get(0))?;pending.insert(0,(middle,to));
    tx.execute("UPDATE t212_sync_run SET phase='request_report',status='queued',report_from=$3,report_to=$4,report_id=NULL,report_requested_at=NULL,pending_intervals=$5,next_attempt_at=clock_timestamp(),error_code=NULL,error_message=NULL WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&from,&middle,&serde_json::to_value(pending)?]).await?;
    tx.commit().await?;Ok(true)
}
