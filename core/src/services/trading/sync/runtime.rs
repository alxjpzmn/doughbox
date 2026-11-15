use std::{collections::HashSet, future::Future, time::Duration};

use anyhow::Context;
use chrono::TimeDelta;
use tokio::{sync::watch,task::{JoinHandle,JoinSet},time::{interval,MissedTickBehavior}};
use tokio_postgres::Client;

use super::{activity, store, T212ImportContext,T212ImportSource};
use crate::{database::{db_client,db_connection,models::auth::UserId},services::{importers::trading212::{apply_trading212_batch,prepare_trading212_csv,PreparedT212Batch},trading::{provider::{self,history::{HistoryHttpError,HistoryReportStatus,download::{download_report,ReportTooLarge}},BrokerClient},rate_limits::{BrokerAccess,BrokerRateLimited,BrokerRequestContext,AdmissionMode,connection_scope}}}};

/// Independent API-owned runtime. A browser request only enqueues SQL state.
pub struct T212SyncRuntime {stop:watch::Sender<bool>,task:JoinHandle<()>}
impl T212SyncRuntime {
    pub fn start()->Self {let (stop,receiver)=watch::channel(false);Self{stop,task:tokio::spawn(scheduler(receiver))}}
    pub async fn shutdown(self){let _=self.stop.send(true);let _=self.task.await;}
}

async fn scheduler(mut stop:watch::Receiver<bool>){
    let mut schedule_tick=interval(Duration::from_secs(60));schedule_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut work_tick=interval(Duration::from_secs(1));work_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut workers=JoinSet::new();let mut active=HashSet::new();
    loop {tokio::select!{
        biased;
        _=stop.changed()=>break,
        result=workers.join_next(),if !workers.is_empty()=>{if let Some(Ok(identity))=result {active.remove(&identity);}},
        _=schedule_tick.tick()=>{
            if let Ok(mut db)=db_client().await {if store::schedule_due(&mut db,store::now()).await.is_err(){tracing::error!("Could not schedule Trading 212 activity sync");}}
        },
        _=work_tick.tick()=>{
            if provider::check_master_key().is_err(){continue;}
            if let Ok(db)=db_client().await {if let Ok(due)=store::due_steps(&db).await {
                for (user,account) in due {
                    let identity=(user.clone(),account.clone());if !active.insert(identity.clone()){continue;}
                    let stop=stop.clone();workers.spawn(async move {if run_session(&user,&account,stop).await.is_err(){tracing::warn!("Trading 212 sync step stopped; durable state retained for recovery");}identity});
                }
            }}
        }
    }}
    // Every parser is cooperatively cancelled and joined before its DB session
    // can be released; shutdown never detaches blocking parsing work.
    while workers.join_next().await.is_some(){}
}

pub(crate) async fn run_session(user_id:&UserId,account_id:&str,stop:watch::Receiver<bool>)->anyhow::Result<()> {
    let (mut db,connection)=db_connection().await?;tokio::pin!(connection);
    let (lost,receiver)=watch::channel(false);
    let work=run_step(user_id,account_id,&mut db,stop,receiver);tokio::pin!(work);
    tokio::select!{biased;
        _=&mut connection=>{let _=lost.send(true);let _=work.await;anyhow::bail!("Trading 212 sync database ownership was lost")},
        result=&mut work=>result,
    }
}

async fn cancelable<T>(future:impl Future<Output=anyhow::Result<T>>,stop:&mut watch::Receiver<bool>,lost:&mut watch::Receiver<bool>)->anyhow::Result<T>{
    if *stop.borrow() || *lost.borrow(){anyhow::bail!("Trading 212 sync stopped before accepting broker data");}
    tokio::select!{biased;
        _=lost.changed()=>anyhow::bail!("Trading 212 sync database ownership was lost"),
        _=stop.changed()=>anyhow::bail!("Trading 212 sync stopped before accepting broker data"),
        result=future=>result,
    }
}

async fn run_step(user_id:&UserId,account_id:&str,db:&mut Client,mut stop:watch::Receiver<bool>,mut lost:watch::Receiver<bool>)->anyhow::Result<()> {
    if *stop.borrow() || *lost.borrow(){return Ok(());}
    let Some(mut step)=store::claim_step(user_id,account_id,db).await? else{return Ok(())};
    let credentials=match provider::decrypt_credentials(&step.credentials) {
        Ok(credentials)=>credentials,
        Err(_)=>return store::fail(user_id,db,&step,"credentials_unavailable","The connected credentials could not be decrypted. Restore the correct server encryption key or reconnect Trading 212.").await,
    };
    let scope=connection_scope(&step.environment,&step.account,&credentials).await?;
    let broker=BrokerClient::new(&step.environment,credentials,BrokerRequestContext{scope,mode:AdmissionMode::Try,access:BrokerAccess::Owner{user_id:user_id.clone(),generation:step.generation.clone()}})?;
    let result=perform_step(user_id,db,&mut step,&broker,&mut stop,&mut lost).await;
    if *stop.borrow() || *lost.borrow(){return Ok(());}
    if let Err(error)=result {
        if let Some(rate)=error.downcast_ref::<BrokerRateLimited>() {
            return store::defer(user_id,db,&step,rate.retry_after_seconds,"rate_limited","Waiting for the shared Trading 212 account request budget.").await;
        }
        if let Some(http)=error.downcast_ref::<HistoryHttpError>() {
            if http.code=="invalid_schema" || (400..500).contains(&http.status) && http.status!=408 {
                return store::fail(user_id,db,&step,http.code,&http.message).await;
            }
        }
        if step.run.phase=="request_report" {
            return store::defer(user_id,db,&step,31,"request_unconfirmed","Report acknowledgement was not received. The next step will reconcile existing reports before requesting another.").await;
        }
        if step.run.phase=="poll_report" {
            return store::defer(user_id,db,&step,61,"status_unavailable","Report status could not be read. Retrying at the next shared status slot; prior coverage is unchanged.").await;
        }
        // Download/parse/transaction errors never advance the report checkpoint.
        return store::fail(user_id,db,&step,"report_import_failed","The report could not be downloaded, parsed, or committed. Check its format and server/provider availability, then retry; prior imported chunks are retained.").await;
    }
    Ok(())
}

async fn perform_step(user_id:&UserId,db:&mut Client,step:&mut store::Step,broker:&BrokerClient,stop:&mut watch::Receiver<bool>,lost:&mut watch::Receiver<bool>)->anyhow::Result<()> {
    let from=step.run.report_from.context("Missing report interval")?;let to=step.run.report_to.context("Missing report interval")?;
    if activity::history_feed(&step.run.phase).is_some() || step.run.report_id.as_deref()==Some(activity::HISTORY_COMPLETE) {
        return history_lists(user_id,db,step,broker,from,to,stop,lost).await;
    }
    if step.run.phase=="request_report" {
        if step.requested_at.is_some() {
            let Some(reports)=read_reports(user_id,db,step,broker,stop,lost).await? else{return Ok(())};
            let matching:Vec<_>=reports.iter().filter(|r|r.matches(from,to)).collect();
            if matching.len()==1 {
                publish_report(user_id,db,step,&matching[0].report_id).await?;return Ok(());
            }
        }
        {
            let tx=store::step_transaction(user_id,db,step).await?;
            tx.execute("UPDATE t212_sync_run SET report_requested_at=clock_timestamp() WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id]).await?;tx.commit().await?;
        }
        match cancelable(broker.request_history_report(from,to),stop,lost).await {
            Ok(id)=>publish_report(user_id,db,step,&id).await,
            Err(error)=>{
                if error.downcast_ref::<HistoryHttpError>().is_some_and(HistoryHttpError::allows_history_list_fallback) {
                    return store::switch_to_history(user_id,db,step).await;
                }
                if error.downcast_ref::<BrokerRateLimited>().is_some() {
                    let tx=store::step_transaction(user_id,db,step).await?;
                    tx.execute("UPDATE t212_sync_run SET report_requested_at=NULL WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id]).await?;tx.commit().await?;
                }
                if error.downcast_ref::<HistoryHttpError>().is_some_and(|e|e.status==400) {
                    if store::split(user_id,db,step,86400).await?{return Ok(());}
                    return store::fail(user_id,db,step,"report_range_rejected","Trading 212 rejected the requested history/filter interval at one day or less. The failed range is retained; verify the broker's supported history boundary and history/export access.").await;
                }
                Err(error)
            }
        }
    } else {
        let Some(reports)=read_reports(user_id,db,step,broker,stop,lost).await? else{return Ok(())};
        let id=step.run.report_id.as_deref().context("Missing acknowledged report ID")?;
        let Some(report)=reports.into_iter().find(|r|r.report_id==id) else {
            return store::fail(user_id,db,step,"report_missing","Trading 212 no longer lists the requested report. No empty-account assumption was made; start another sync.").await;
        };
        if !report.matches(from,to) {
            return store::fail(user_id,db,step,"report_coverage_mismatch","Trading 212 report bounds or included activity differ from the requested interval. Coverage has not advanced.").await;
        }
        match report.status {
            HistoryReportStatus::Queued|HistoryReportStatus::Processing|HistoryReportStatus::Running=>{
                if step.requested_at.is_some_and(|at|store::now()-at>=TimeDelta::hours(24)) {
                    store::fail(user_id,db,step,"report_expired","Trading 212 did not finish the report within 24 hours. Start another sync; coverage has not advanced.").await
                } else {store::defer(user_id,db,step,61,"report_processing","Trading 212 is generating the report.").await}
            },
            HistoryReportStatus::Canceled|HistoryReportStatus::Failed=>store::fail(user_id,db,step,"report_failed","Trading 212 canceled or failed this report. Check history/export access and retry; prior coverage is unchanged.").await,
            HistoryReportStatus::Finished=>{
                let Some(link)=report.download_link.filter(|link|!link.trim().is_empty()) else {
                    return store::fail(user_id,db,step,"report_download_missing","Trading 212 marked the report finished without a download link. Start another sync.").await;
                };
                phase(user_id,db,step,"download_report").await?;
                broker.authorize(true).await?;
                let bytes=match cancelable(download_report(&link),stop,lost).await {
                    Ok(bytes)=>bytes,
                    Err(error) if error.downcast_ref::<ReportTooLarge>().is_some()=>{
                        if store::split(user_id,db,step,1).await? {return Ok(());}
                        return store::fail(user_id,db,step,"report_too_large","A single one-second timestamp bucket exceeds the 25 MiB report limit. No activity from that report was applied.").await;
                    }
                    Err(error)=>return Err(error),
                };
                broker.authorize(true).await?;
                phase(user_id,db,step,"import_report").await?;
                let prepared=prepare_owned(user_id.clone(),bytes,stop.clone(),lost.clone()).await?;
                if *stop.borrow() || *lost.borrow(){anyhow::bail!("Sync stopped before financial application");}
                // This is exactly the session that owns the sync-step lock.
                apply_trading212_batch(user_id,db,prepared,&T212ImportContext{source:T212ImportSource::Trading212Api,account_id:Some(step.run.account_id.clone()),sync_run_id:Some(step.run.id.clone()),statement_job_id:None}).await?;
                Ok(())
            }
        }
    }
}

async fn history_lists(user_id:&UserId,db:&mut Client,step:&mut store::Step,broker:&BrokerClient,from:chrono::DateTime<chrono::Utc>,to:chrono::DateTime<chrono::Utc>,stop:&mut watch::Receiver<bool>,lost:&mut watch::Receiver<bool>)->anyhow::Result<()> {
    if step.run.report_id.as_deref()==Some(activity::HISTORY_COMPLETE) {return store::complete_history(user_id,db,step).await;}
    let feed=activity::history_feed(&step.run.phase).context("Missing history feed")?;
    let path=step.run.report_id.as_deref().filter(|id| id.starts_with("/equity/history/")).map(str::to_owned).unwrap_or_else(|| format!("/equity/history/{feed}?limit=50"));
    let page=cancelable(broker.history_activity_page(&path),stop,lost).await?;
    let instruments=db.query_one("SELECT instruments FROM trading_connection WHERE user_id=$1 AND generation=$2",&[&user_id.as_str(),&step.generation]).await?.get::<_, serde_json::Value>(0);
    let csv=activity::history_csv(feed,&page.items,from,to,&instruments);
    if csv.lines().nth(1).is_some() {
        let prepared=prepare_trading212_csv(user_id,&csv.into_bytes()).await?;
        if *stop.borrow() || *lost.borrow(){anyhow::bail!("Sync stopped before financial application");}
        apply_trading212_batch(user_id,db,prepared,&T212ImportContext{source:T212ImportSource::Trading212Api,account_id:Some(step.run.account_id.clone()),sync_run_id:Some(step.run.id.clone()),statement_job_id:None}).await?;
    }
    let next=activity::next_history_path(feed,page.next_page_path.as_deref())?;
    if let Some(next)=next {
        return store::save_history_cursor(user_id,db,step,&step.run.phase,Some(&next)).await;
    }
    if let Some(phase)=activity::next_history_phase(&step.run.phase) {
        return store::save_history_cursor(user_id,db,step,phase,None).await;
    }
    store::save_history_cursor(user_id,db,step,&step.run.phase,Some(activity::HISTORY_COMPLETE)).await?;
    store::complete_history(user_id,db,step).await
}

async fn phase(user_id:&UserId,db:&mut Client,step:&mut store::Step,phase:&str)->anyhow::Result<()> {
    let tx=store::step_transaction(user_id,db,step).await?;
    tx.execute("UPDATE t212_sync_run SET phase=$3 WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&phase]).await?;tx.commit().await?;step.run.phase=phase.into();Ok(())
}
async fn publish_report(user_id:&UserId,db:&mut Client,step:&store::Step,id:&str)->anyhow::Result<()> {
    let tx=store::step_transaction(user_id,db,step).await?;
    tx.execute("UPDATE t212_sync_run SET report_id=$3,phase='poll_report',status='waiting',next_attempt_at=clock_timestamp(),error_code=NULL,error_message=NULL WHERE user_id=$1 AND id=$2",&[&user_id.as_str(),&step.run.id,&id]).await?;tx.commit().await?;Ok(())
}

struct Preparation {cancel:watch::Sender<bool>,task:JoinHandle<anyhow::Result<PreparedT212Batch>>}
impl Drop for Preparation {fn drop(&mut self){let _=self.cancel.send(true);}}
async fn prepare_owned(user_id:UserId,bytes:Vec<u8>,mut stop:watch::Receiver<bool>,mut lost:watch::Receiver<bool>)->anyhow::Result<PreparedT212Batch> {
    let (cancel,mut receiver)=watch::channel(false);
    let task=tokio::task::spawn_blocking(move || {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().context("Could not start report parser")?;
        runtime.block_on(async {if *receiver.borrow(){anyhow::bail!("Report parsing cancelled");}tokio::select!{biased;
            _=receiver.changed()=>anyhow::bail!("Report parsing cancelled"),
            result=prepare_trading212_csv(&user_id,&bytes)=>result,
        }})
    });
    let mut parser=Preparation{cancel,task};
    let result=tokio::select!{biased;
        _=lost.changed()=>None,
        _=stop.changed()=>None,
        result=&mut parser.task=>Some(result),
    };
    match result {Some(result)=>result.context("Report parser stopped unexpectedly")?,None=>{let _=parser.cancel.send(true);let _=(&mut parser.task).await;anyhow::bail!("Report parsing cancelled before releasing database ownership")}}
}

async fn read_reports(user_id:&UserId,db:&mut Client,step:&store::Step,broker:&BrokerClient,stop:&mut watch::Receiver<bool>,lost:&mut watch::Receiver<bool>)->anyhow::Result<Option<Vec<provider::history::HistoryReport>>> {
    match cancelable(broker.history_reports(),stop,lost).await {
        Ok(reports)=>Ok(Some(reports)),
        Err(error)=>{
            let definite=error.downcast_ref::<BrokerRateLimited>().is_some() || error.downcast_ref::<HistoryHttpError>()
                .is_some_and(|http|http.code=="invalid_schema" || (400..500).contains(&http.status) && http.status!=408);
            if definite {return Err(error);}
            if *stop.borrow() || *lost.borrow(){return Err(error);}
            store::defer(user_id,db,step,61,"status_unavailable","Report status could not be read. Retrying at the next shared status slot; prior coverage is unchanged.").await?;
            Ok(None)
        }
    }
}
