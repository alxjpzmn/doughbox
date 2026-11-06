use anyhow::{ensure, Context};
use serde_json::Value;
use tokio_postgres::{Client, Row, Transaction};

use super::{Proposal, TradeResult};
use crate::database::db_client;

// Session-scoped lock spans broker calls, across processes. No transaction is held over HTTP.
const EXECUTION_LOCK: i64 = 0x444F5547485452;
pub(super) async fn execution_lock(client: &Client) -> anyhow::Result<bool> {
    Ok(client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&EXECUTION_LOCK])
        .await?
        .get(0))
}
pub(super) async fn recover_interrupted() -> anyhow::Result<()> {
    let client = db_client().await?;
    if execution_lock(&client).await? {
        recover_on(&client).await?;
    }
    Ok(())
}
pub(super) async fn recover_on(client: &Client) -> anyhow::Result<()> {
    // Called only while holding the execution lock. A surviving worker owns that lock;
    // otherwise any submitting leg might already have reached the broker and must never replay.
    for row in client
        .query(
            "SELECT * FROM trading_proposals WHERE status='executing'",
            &[],
        )
        .await?
    {
        let mut proposal = Proposal::from_row(&row)?;
        for trade in &mut proposal.trades {
            match trade.status.as_str() {
                "submitting" => {
                    trade.status = "unknown".into();
                    trade.error = Some("Execution was interrupted. Check Trading 212 before submitting another trade; this order may have been accepted.".into());
                }
                "pending" => {
                    trade.status = "discarded".into();
                    trade.error = Some("Not sent: execution was interrupted.".into());
                }
                _ => {}
            }
        }
        client
            .execute(
                "UPDATE trading_proposals SET trades=$2,status='completed' WHERE id=$1",
                &[&proposal.id, &serde_json::to_value(proposal.trades)?],
            )
            .await?;
    }
    Ok(())
}

pub(super) async fn replace_connection(
    connection: Option<(&str, &str, &Value, &Value)>,
) -> anyhow::Result<()> {
    let mut client = db_client().await?;
    ensure!(
        execution_lock(&client).await?,
        "A proposal is executing. Wait before changing the connection."
    );
    recover_on(&client).await?;
    let tx = client.transaction().await?;
    tx.query_one(
        "SELECT singleton FROM trading_connection WHERE singleton FOR UPDATE",
        &[],
    )
    .await?;
    for row in tx
        .query(
            "SELECT * FROM trading_proposals WHERE status='pending' FOR UPDATE",
            &[],
        )
        .await?
    {
        let mut proposal = Proposal::from_row(&row)?;
        discard_trades(
            &mut proposal.trades,
            "Connection changed; proposal discarded.",
        );
        tx.execute(
            "UPDATE trading_proposals SET status='discarded', trades=$2 WHERE id=$1",
            &[&proposal.id, &serde_json::to_value(proposal.trades)?],
        )
        .await?;
    }
    let generation = uuid::Uuid::new_v4().to_string();
    match connection {
        Some((environment, credentials, account, instruments)) => {
            tx.execute("UPDATE trading_connection SET generation=$1,environment=$2,credentials=$3,account=$4,instruments=$5,agent_token_hash=NULL,last_instruments_request_at=clock_timestamp() WHERE singleton", &[&generation,&environment,&credentials,&account,&instruments]).await?;
        }
        None => {
            tx.execute("UPDATE trading_connection SET generation=$1,environment=NULL,credentials=NULL,account=NULL,instruments='[]',agent_token_hash=NULL WHERE singleton", &[&generation]).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn agent_connection(tx: &Transaction<'_>, hash: &str) -> anyhow::Result<Row> {
    super::check_setup()?;
    tx.query_opt("SELECT * FROM trading_connection WHERE singleton AND credentials IS NOT NULL AND agent_token_hash=$1 FOR UPDATE", &[&hash]).await?.context("Invalid or revoked trading token.")
}

fn discard_trades(trades: &mut [TradeResult], reason: &str) {
    for trade in trades {
        trade.status = "discarded".into();
        trade.error = Some(reason.into());
    }
}
pub(super) async fn discard(id: &str) -> anyhow::Result<Proposal> {
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    // Match approval's lock order to prevent deadlocks.
    tx.query_one(
        "SELECT singleton FROM trading_connection WHERE singleton FOR UPDATE",
        &[],
    )
    .await?;
    let row = tx
        .query_opt(
            "SELECT * FROM trading_proposals WHERE id=$1 FOR UPDATE",
            &[&id],
        )
        .await?
        .context("Proposal not found.")?;
    let mut proposal = Proposal::from_row(&row)?;
    ensure!(
        proposal.status == "pending",
        "Only pending proposals can be discarded. Submitted orders must be managed in Trading 212."
    );
    proposal.status = "discarded".into();
    discard_trades(
        &mut proposal.trades,
        "Discarded by user; not sent to broker.",
    );
    tx.execute(
        "UPDATE trading_proposals SET status='discarded',trades=$2 WHERE id=$1",
        &[&id, &serde_json::to_value(&proposal.trades)?],
    )
    .await?;
    tx.commit().await?;
    Ok(proposal)
}
pub(super) async fn save_trades(
    client: &Client,
    id: &str,
    trades: &[TradeResult],
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE trading_proposals SET trades=$2 WHERE id=$1 AND status='executing'",
            &[&id, &serde_json::to_value(trades)?],
        )
        .await?;
    Ok(())
}

pub(super) async fn pace_submission(client: &Client) -> anyhow::Result<()> {
    // Limit placement is documented at 1 request / 2s. Three seconds avoids the window edge.
    let delay: f64 = client.query_one(
        "SELECT GREATEST(0, 3 - EXTRACT(EPOCH FROM (clock_timestamp()-last_submission_at)))::float8 FROM trading_connection WHERE singleton",
        &[],
    ).await?.get(0);
    tokio::time::sleep(std::time::Duration::from_secs_f64(delay.min(3.0))).await;
    Ok(())
}

pub(super) async fn mark_submission(client: &Client) -> anyhow::Result<()> {
    client.execute("UPDATE trading_connection SET last_submission_at=clock_timestamp() WHERE singleton", &[]).await?;
    Ok(())
}

pub(super) async fn claim_poll(client: &Client, column: &str, interval: &str) -> anyhow::Result<bool> {
    // column and interval are fixed literals from the caller, never request data.
    let claimed = client.query_opt(
        &format!("UPDATE trading_connection SET {column}=clock_timestamp() WHERE singleton AND {column} <= clock_timestamp() - interval '{interval}' RETURNING 1"),
        &[],
    ).await?;
    Ok(claimed.is_some())
}

pub(super) async fn reconcile_order_statuses(
    client: &Client,
    id: &str,
    statuses: &std::collections::HashMap<String, String>,
) -> anyhow::Result<Proposal> {
    // Merge only broker-owned fields against the locked UPDATE target, not a snapshot
    // taken before HTTP. Execution owns the entire trade until the proposal completes.
    let statuses = serde_json::to_value(statuses)?;
    let updated = client
        .query_opt(
            "UPDATE trading_proposals AS proposal
             SET trades=(
                 SELECT jsonb_agg(
                     CASE WHEN trade->>'status' IN ('submitted', 'open', 'partial')
                               AND $2::jsonb ? (trade->>'broker_order_id')
                               AND trade->>'status' <> $2::jsonb->>(trade->>'broker_order_id')
                          THEN trade || jsonb_build_object(
                              'status', $2::jsonb->>(trade->>'broker_order_id'), 'error', NULL)
                          ELSE trade END ORDER BY position)
                 FROM jsonb_array_elements(proposal.trades) WITH ORDINALITY AS legs(trade, position)
             )
             WHERE proposal.id=$1 AND proposal.status <> 'executing'
               AND EXISTS (
                   SELECT 1 FROM jsonb_array_elements(proposal.trades) AS legs(trade)
                   WHERE trade->>'status' IN ('submitted', 'open', 'partial')
                     AND $2::jsonb ? (trade->>'broker_order_id')
                     AND trade->>'status' <> $2::jsonb->>(trade->>'broker_order_id')
               )
             RETURNING proposal.*",
            &[&id, &statuses],
        )
        .await?;
    let row = match updated {
        Some(row) => row,
        None => client
            .query_opt("SELECT * FROM trading_proposals WHERE id=$1", &[&id])
            .await?
            .context("Proposal not found.")?,
    };
    Proposal::from_row(&row)
}
