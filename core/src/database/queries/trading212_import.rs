use anyhow::{ensure, Context};
use serde_json::Value;
use tokio_postgres::Transaction;

use crate::{
    database::{models::auth::UserId, queries::auth},
    services::{auth::AuthError, trading::sync::{T212ImportContext, T212ImportSource}},
};

/// Every T212 write starts here, before connection, ingestion, run or event locks.
pub async fn lock_owner(user_id: &UserId, tx: &Transaction<'_>) -> anyhow::Result<()> {
    ensure!(auth::lock_user(user_id, tx).await?.fully_enrolled(), AuthError::Unauthenticated);
    Ok(())
}

pub async fn lock_ingestion(user_id: &UserId, tx: &Transaction<'_>) -> anyhow::Result<()> {
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('doughbox:t212-import:' || $1::text, 0))",
        &[&user_id.as_str()],
    ).await?;
    Ok(())
}

/// Called only after successful broker verification, with owner/connection locked.
/// Stable account identity is independent of credentials and connection generation.
pub async fn connection_changed(
    user_id: &UserId,
    tx: &Transaction<'_>,
    connection: Option<(&str, &Value)>,
) -> anyhow::Result<Option<String>> {
    let current = match connection {
        Some((environment @ ("demo" | "live"), account)) => account.get("id").and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty()).map(|id| (environment, id, account)),
        _ => None,
    };
    let account_id = if let Some((environment, broker_id, account)) = current {
        let id = uuid::Uuid::new_v4().to_string();
        let currency = account.get("currency").and_then(Value::as_str).unwrap_or("");
        Some(tx.query_one(
            "INSERT INTO t212_sync_account(user_id,id,environment,broker_account_id,currency) VALUES($1,$2,$3,$4,$5)
             ON CONFLICT(user_id,environment,broker_account_id) DO UPDATE SET currency=EXCLUDED.currency
             RETURNING id",
            &[&user_id.as_str(), &id, &environment, &broker_id, &currency],
        ).await?.get::<_, String>(0))
    } else { None };
    // Switching away pauses old accounts. Reconnecting the same account leaves its
    // saved schedule/checkpoints intact; a return to a former account stays paused.
    tx.execute(
        "UPDATE t212_sync_account SET schedule_enabled=false,next_run_at=NULL
         WHERE user_id=$1 AND ($2::text IS NULL OR id<>$2)",
        &[&user_id.as_str(), &account_id],
    ).await?;
    tx.execute(
        "UPDATE t212_sync_run SET status='cancelled',finished_at=clock_timestamp(),
             error_code='connection_changed',error_message='Connection changed. Start a new sync with the current connection.'
         WHERE user_id=$1 AND status IN ('queued','running','waiting','needs_review')",
        &[&user_id.as_str()],
    ).await?;
    Ok(account_id)
}

/// Lock and validate the API fence before taking ingestion/run/account locks.
/// Reviews also call this when they would advance an API checkpoint.
pub async fn validate_context(
    user_id: &UserId,
    tx: &Transaction<'_>,
    context: &T212ImportContext,
) -> anyhow::Result<()> {
    if context.source == T212ImportSource::Trading212Api {
        let account_id = context.account_id.as_deref().context("API import requires a connected account")?;
        let run_id = context.sync_run_id.as_deref().context("API import requires a sync run")?;
        let connection = tx.query_opt(
            "SELECT generation,environment,account FROM trading_connection WHERE user_id=$1
             AND environment IN ('demo','live') AND credentials IS NOT NULL FOR UPDATE",
            &[&user_id.as_str()],
        ).await?.context("Trading 212 connection is no longer available")?;
        let valid = tx.query_opt(
            "SELECT 1 FROM t212_sync_run r JOIN t212_sync_account a ON a.user_id=r.user_id AND a.id=r.account_id
             WHERE r.user_id=$1 AND r.id=$2 AND r.account_id=$3 AND r.connection_generation=$4
             AND r.status IN ('queued','running','waiting','needs_review')
             AND a.broker_account_id=$5 AND a.environment=$6",
            &[&user_id.as_str(), &run_id, &account_id, &connection.get::<_, String>("generation"),
                &connection.get::<_, Value>("account").get("id").and_then(Value::as_str),
                &connection.get::<_, String>("environment")],
        ).await?;
        ensure!(valid.is_some(), "Trading 212 sync connection changed or run is no longer active");
    } else {
        ensure!(context.sync_run_id.is_none(), "Manual import cannot select a sync run");
        if let Some(account_id) = context.account_id.as_deref() {
            ensure!(tx.query_opt("SELECT 1 FROM t212_sync_account WHERE user_id=$1 AND id=$2",
                &[&user_id.as_str(), &account_id]).await?.is_some(), AuthError::NotFound);
        }
    }
    Ok(())
}

/// Recompute from durable observations so parser/decision replays cannot inflate
/// counters. A report checkpoint belongs to the financial transaction.
pub async fn finish_batch(
    user_id: &UserId, tx: &Transaction<'_>, batch_id: &str,
) -> anyhow::Result<crate::services::trading::sync::T212ImportOutcome> {
    let batch = tx.query_one(
        "UPDATE t212_import_batch b SET
             inserted_count=c.inserted,matched_count=c.matched,review_count=c.review,
             status=CASE WHEN c.review>0 THEN 'needs_review' ELSE 'applied' END,
             applied_at=CASE WHEN c.review=0 THEN COALESCE(b.applied_at,clock_timestamp()) ELSE NULL END
         FROM (SELECT count(*) FILTER(WHERE state='inserted')::int AS inserted,
                      count(*) FILTER(WHERE state='matched')::int AS matched,
                      count(*) FILTER(WHERE state IN ('pending','review'))::int AS review
               FROM t212_import_row WHERE user_id=$1 AND batch_id=$2) c
         WHERE b.user_id=$1 AND b.id=$2 RETURNING b.*",
        &[&user_id.as_str(), &batch_id],
    ).await?;
    if let Some(run_id) = batch.get::<_, Option<String>>("sync_run_id") {
        tx.execute(
            "UPDATE t212_sync_run r SET inserted_count=c.inserted,matched_count=c.matched,
                 review_count=c.review,skipped_count=c.skipped
             FROM (SELECT COALESCE(sum(inserted_count),0)::int AS inserted,
                          COALESCE(sum(matched_count),0)::int AS matched,
                          COALESCE(sum(review_count),0)::int AS review,
                          COALESCE(sum(skipped_count),0)::int AS skipped
                   FROM t212_import_batch WHERE user_id=$1 AND sync_run_id=$2) c
             WHERE r.user_id=$1 AND r.id=$2",
            &[&user_id.as_str(), &run_id],
        ).await?;
        // A resolved historical batch can remain useful evidence even after
        // reconnection, but must never move a cancelled/stale run's coverage.
        let run = tx.query_opt(
            "SELECT r.* FROM t212_sync_run r
             JOIN trading_connection c ON c.user_id=r.user_id AND c.generation=r.connection_generation
             JOIN t212_sync_account a ON a.user_id=r.user_id AND a.id=r.account_id
             WHERE r.user_id=$1 AND r.id=$2 AND r.status IN ('queued','running','waiting','needs_review')
               AND c.environment IN ('demo','live') AND c.credentials IS NOT NULL AND c.environment=a.environment AND c.account->>'id'=a.broker_account_id
             FOR UPDATE OF r",
            &[&user_id.as_str(), &run_id],
        ).await?;
        if let Some(run) = run {
            if batch.get::<_, i32>("review_count") > 0 {
                tx.execute("UPDATE t212_sync_run SET status='needs_review' WHERE user_id=$1 AND id=$2",
                    &[&user_id.as_str(), &run_id]).await?;
            } else if run.get::<_, String>("phase").starts_with("history_") {
                // A history-list page shares the run interval, but coverage moves
                // only after every feed has been applied.
                tx.execute("UPDATE t212_sync_run SET status='queued',error_code=NULL,error_message=NULL,next_attempt_at=clock_timestamp() WHERE user_id=$1 AND id=$2 AND status='needs_review'",
                    &[&user_id.as_str(), &run_id]).await?;
            } else {
                let to: Option<chrono::DateTime<chrono::Utc>> = batch.get("report_to");
                let from: Option<chrono::DateTime<chrono::Utc>> = batch.get("report_from");
                if to == run.get("report_to") && from == run.get("report_from") && to.is_some() {
                    let to = to.expect("checked report interval");
                    let complete = to >= run.get::<_, chrono::DateTime<chrono::Utc>>("range_to");
                    let account_id: String = run.get("account_id");
                    tx.execute(
                        "UPDATE t212_sync_run SET cursor=$3,
                             status=CASE WHEN $4 THEN 'succeeded' ELSE 'queued' END,
                             phase='request_report',report_id=NULL,report_from=NULL,report_to=NULL,
                             report_requested_at=NULL,next_attempt_at=clock_timestamp(),
                             finished_at=CASE WHEN $4 THEN clock_timestamp() ELSE NULL END,
                             error_code=NULL,error_message=NULL
                         WHERE user_id=$1 AND id=$2",
                        &[&user_id.as_str(), &run_id, &to, &complete],
                    ).await?;
                    tx.execute(
                        "UPDATE t212_sync_account SET
                             backfill_cursor=GREATEST(backfill_cursor,$3),
                             backfill_complete=backfill_complete OR $4,
                             synced_through=CASE WHEN $4 THEN GREATEST(synced_through,$3) ELSE synced_through END,
                             last_success_at=CASE WHEN $4 THEN clock_timestamp() ELSE last_success_at END
                         WHERE user_id=$1 AND id=$2",
                        &[&user_id.as_str(), &account_id, &to, &complete],
                    ).await?;
                }
            }
        }
    }
    Ok(crate::services::trading::sync::T212ImportOutcome {
        batch_id: batch.get("id"),
        inserted_count: batch.get("inserted_count"),
        matched_count: batch.get("matched_count"),
        review_count: batch.get("review_count"),
        skipped_count: batch.get("skipped_count"),
    })
}
