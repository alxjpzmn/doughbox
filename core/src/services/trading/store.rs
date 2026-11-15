use anyhow::{ensure, Context};
use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio_postgres::{Client, Row, Transaction};

use super::{pies::PieStatus, Proposal, ProposalContent, TradeResult};
use crate::{
    database::{db_client, models::auth::{AuthSession, CurrentUser, UserId}, queries::auth},
    services::auth::{require_browser, require_recent, session_store, AuthError},
};

// A dedicated session owns this per-account lock across HTTP, never a transaction.
pub(super) async fn execution_lock(user_id: &UserId, client: &Client) -> anyhow::Result<bool> {
    Ok(client.query_one(
        "SELECT pg_try_advisory_lock(hashtextextended('doughbox:trading:' || $1::text, 0))",
        &[&user_id.as_str()],
    ).await?.get(0))
}

pub(super) async fn recover_interrupted(user_id: &UserId) -> anyhow::Result<()> {
    let client = db_client().await?;
    if execution_lock(user_id, &client).await? {
        recover_on(user_id, &client).await?;
    }
    Ok(())
}

pub(super) async fn recover_on(user_id: &UserId, client: &Client) -> anyhow::Result<()> {
    // A surviving executor owns this owner's lock. A submitting leg must never replay.
    for row in client.query(
        "SELECT * FROM trading_proposals WHERE user_id=$1 AND status='executing'",
        &[&user_id.as_str()],
    ).await? {
        let mut proposal = Proposal::from_row(&row)?;
        match &mut proposal.content {
            ProposalContent::Trades { trades } => {
                for trade in trades.iter_mut() {
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
                client.execute("UPDATE trading_proposals SET trades=$3,status='completed' WHERE user_id=$1 AND id=$2 AND kind='trades'", &[&user_id.as_str(), &proposal.id, &serde_json::to_value(trades)?]).await?;
            }
            ProposalContent::Pie { pie } => {
                match pie.status {
                    PieStatus::Submitting => {
                        pie.status = PieStatus::Unknown;
                        pie.error = Some("Execution was interrupted. Check Trading 212 manually; this request may have been accepted. It will not be resent.".into());
                    }
                    PieStatus::Pending => {
                        pie.status = PieStatus::Discarded;
                        pie.error = Some("Not sent: execution was interrupted.".into());
                    }
                    _ => {}
                }
                client.execute("UPDATE trading_proposals SET pie=$3,status='completed' WHERE user_id=$1 AND id=$2 AND kind='pie'", &[&user_id.as_str(), &proposal.id, &serde_json::to_value(pie)?]).await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn replace_connection(
    user_id: &UserId,
    connection: Option<(&str, &str, &Value, &Value)>,
    expected_generation: Option<&str>,
) -> anyhow::Result<()> {
    let mut client = db_client().await?;
    ensure!(execution_lock(user_id, &client).await?, "A proposal is executing. Wait before changing the connection.");
    recover_on(user_id, &client).await?;
    let tx = client.transaction().await?;
    replace_connection_on(user_id, &tx, connection, expected_generation).await?;
    tx.commit().await?;
    Ok(())
}

// Caller owns the account's execution lock before acquiring connection/proposal rows.
pub(super) async fn replace_connection_on(
    user_id: &UserId,
    tx: &Transaction<'_>,
    connection: Option<(&str, &str, &Value, &Value)>,
    expected_generation: Option<&str>,
) -> anyhow::Result<()> {
    crate::database::queries::trading212_import::lock_owner(user_id, tx).await?;
    let current = tx.query_one("SELECT generation FROM trading_connection WHERE user_id=$1 FOR UPDATE", &[&user_id.as_str()]).await?;
    if let Some(expected) = expected_generation {
        ensure!(current.get::<_, &str>("generation") == expected, "Trading connection changed during verification. Connect again.");
    }
    crate::database::queries::trading212_import::connection_changed(
        user_id, tx, connection.map(|(environment, _, account, _)| (environment, account)),
    ).await?;
    for row in tx.query(
        "SELECT * FROM trading_proposals WHERE user_id=$1 AND status='pending' FOR UPDATE",
        &[&user_id.as_str()],
    ).await? {
        let mut proposal = Proposal::from_row(&row)?;
        discard_content(&mut proposal.content, "Connection changed; proposal discarded.");
        let (trades, pie) = payload_values(&proposal.content)?;
        tx.execute(
            "UPDATE trading_proposals SET status='discarded', trades=$3, pie=$4 WHERE user_id=$1 AND id=$2",
            &[&user_id.as_str(), &proposal.id, &trades, &pie],
        ).await?;
    }
    let generation = uuid::Uuid::new_v4().to_string();
    match connection {
        Some((environment, credentials, account, instruments)) => {
            tx.execute("UPDATE trading_connection SET generation=$2,environment=$3,credentials=$4,account=$5,instruments=$6,agent_token_hash=NULL,agent_auth_version=NULL WHERE user_id=$1", &[&user_id.as_str(),&generation,&environment,&credentials,&account,&instruments]).await?;
        }
        None => {
            tx.execute("UPDATE trading_connection SET generation=$2,environment=NULL,credentials=NULL,account=NULL,instruments='[]',agent_token_hash=NULL,agent_auth_version=NULL WHERE user_id=$1", &[&user_id.as_str(),&generation]).await?;
        }
    }
    Ok(())
}

pub(super) async fn agent_connection(user_id: &UserId, tx: &Transaction<'_>, hash: &str) -> anyhow::Result<Row> {
    // Account first, then connection: version revocation and token use serialize.
    let user = auth::lock_user(user_id, tx).await?;
    ensure!(user.fully_enrolled(), "Invalid or revoked trading token.");
    tx.query_opt("SELECT * FROM trading_connection WHERE user_id=$1 AND credentials IS NOT NULL AND environment IS NOT NULL AND agent_token_hash=$2 AND agent_auth_version=$3 FOR UPDATE", &[&user_id.as_str(), &hash, &user.auth_version]).await?.context("Invalid or revoked trading token.")
}

pub(super) async fn authenticate_on(client: &Client, hash: &str) -> anyhow::Result<UserId> {
    let row = client.query_opt(
        "SELECT c.user_id FROM trading_connection c JOIN auth_users u ON u.id=c.user_id WHERE c.credentials IS NOT NULL AND c.environment IS NOT NULL AND c.agent_token_hash=$1 AND c.agent_auth_version=u.auth_version AND u.status='active' AND u.username IS NOT NULL AND u.password_hash IS NOT NULL AND u.totp_secret_encrypted IS NOT NULL",
        &[&hash],
    ).await?.context("Invalid or revoked trading token.")?;
    Ok(UserId::from_database(row.get("user_id")))
}

pub(super) async fn agent_proposal(user_id: &UserId, tx: &Transaction<'_>, hash: &str, id: &str) -> anyhow::Result<Proposal> {
    let connection = agent_connection(user_id, tx, hash).await?;
    let generation: String = connection.get("generation");
    let row = tx.query_opt(
        "SELECT * FROM trading_proposals WHERE user_id=$1 AND id=$2 AND generation=$3",
        &[&user_id.as_str(), &id, &generation],
    ).await?.ok_or(AuthError::NotFound)?;
    Proposal::from_row(&row)
}

pub(super) async fn rotate_token_on(
    user_id: &UserId,
    tx: &Transaction<'_>,
    actor: &CurrentUser,
    source: &AuthSession,
    hash: &str,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    require_browser(actor)?;
    if user_id != &actor.id || source.user_id.as_ref() != Some(user_id) || source.auth_version != Some(actor.auth_version) {
        return Err(AuthError::StateChanged);
    }
    auth::lock_actor(tx, actor, false).await?;
    let source = session_store::lock_source(tx, source, now).await?;
    require_recent(&source, now)?;
    if tx.execute("UPDATE trading_connection SET agent_token_hash=$2,agent_auth_version=$3 WHERE user_id=$1 AND credentials IS NOT NULL", &[&user_id.as_str(), &hash, &actor.auth_version]).await? != 1 {
        return Err(AuthError::InvalidInput("Connect Trading 212 first."));
    }
    Ok(())
}

fn discard_trades(trades: &mut [TradeResult], reason: &str) {
    for trade in trades {
        trade.status = "discarded".into();
        trade.error = Some(reason.into());
    }
}

fn discard_content(content: &mut ProposalContent, reason: &str) {
    match content {
        ProposalContent::Trades { trades } => discard_trades(trades, reason),
        ProposalContent::Pie { pie } => {
            pie.status = PieStatus::Discarded;
            pie.error = Some(reason.into());
        }
    }
}

fn payload_values(content: &ProposalContent) -> anyhow::Result<(Value, Option<Value>)> {
    Ok(match content {
        ProposalContent::Trades { trades } => (serde_json::to_value(trades)?, None),
        ProposalContent::Pie { pie } => (serde_json::json!([]), Some(serde_json::to_value(pie)?)),
    })
}

pub(super) async fn discard(user_id: &UserId, id: &str) -> anyhow::Result<Proposal> {
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    // Match approval's connection-then-proposal order.
    tx.query_one("SELECT user_id FROM trading_connection WHERE user_id=$1 FOR UPDATE", &[&user_id.as_str()]).await?;
    let row = tx.query_opt("SELECT * FROM trading_proposals WHERE user_id=$1 AND id=$2 FOR UPDATE", &[&user_id.as_str(), &id]).await?.ok_or(AuthError::NotFound)?;
    let mut proposal = Proposal::from_row(&row)?;
    ensure!(proposal.status == "pending", "Only pending proposals can be discarded. Submitted orders must be managed in Trading 212.");
    proposal.status = "discarded".into();
    discard_content(&mut proposal.content, "Discarded by user; not sent to broker.");
    let (trades, pie) = payload_values(&proposal.content)?;
    tx.execute("UPDATE trading_proposals SET status='discarded',trades=$3,pie=$4 WHERE user_id=$1 AND id=$2", &[&user_id.as_str(), &id, &trades, &pie]).await?;
    tx.commit().await?;
    Ok(proposal)
}

pub(super) async fn save_trades(user_id: &UserId, client: &Client, id: &str, trades: &[TradeResult]) -> anyhow::Result<()> {
    ensure!(client.execute("UPDATE trading_proposals SET trades=$3 WHERE user_id=$1 AND id=$2 AND kind='trades' AND status='executing'", &[&user_id.as_str(), &id, &serde_json::to_value(trades)?]).await? == 1, "Trade execution is no longer current.");
    Ok(())
}

pub(super) async fn reconcile_order_statuses(
    user_id: &UserId,
    client: &(impl tokio_postgres::GenericClient + Sync),
    id: &str,
    statuses: &std::collections::HashMap<String, String>,
) -> anyhow::Result<Proposal> {
    // Merge broker-owned fields against the current target, not a pre-HTTP snapshot.
    let statuses = serde_json::to_value(statuses)?;
    let updated = client.query_opt(
        "UPDATE trading_proposals AS proposal
         SET trades=(
             SELECT jsonb_agg(
                 CASE WHEN trade->>'status' IN ('submitted', 'open', 'partial')
                           AND $3::jsonb ? (trade->>'broker_order_id')
                           AND trade->>'status' <> $3::jsonb->>(trade->>'broker_order_id')
                      THEN trade || jsonb_build_object(
                          'status', $3::jsonb->>(trade->>'broker_order_id'), 'error', NULL)
                      ELSE trade END ORDER BY position)
             FROM jsonb_array_elements(proposal.trades) WITH ORDINALITY AS legs(trade, position)
         )
         WHERE proposal.user_id=$1 AND proposal.id=$2 AND proposal.kind='trades' AND proposal.status <> 'executing'
           AND EXISTS (
               SELECT 1 FROM jsonb_array_elements(proposal.trades) AS legs(trade)
               WHERE trade->>'status' IN ('submitted', 'open', 'partial')
                 AND $3::jsonb ? (trade->>'broker_order_id')
                 AND trade->>'status' <> $3::jsonb->>(trade->>'broker_order_id')
           )
         RETURNING proposal.*",
        &[&user_id.as_str(), &id, &statuses],
    ).await?;
    let row = match updated {
        Some(row) => row,
        None => client.query_opt("SELECT * FROM trading_proposals WHERE user_id=$1 AND id=$2", &[&user_id.as_str(), &id]).await?.ok_or(AuthError::NotFound)?,
    };
    Proposal::from_row(&row)
}

pub(super) async fn browser_connection(user_id: &UserId, tx: &Transaction<'_>) -> anyhow::Result<Row> {
    let user = auth::lock_user(user_id, tx).await?;
    ensure!(user.fully_enrolled(), "The trading owner is no longer authorized.");
    tx.query_opt("SELECT * FROM trading_connection WHERE user_id=$1 AND credentials IS NOT NULL AND environment IS NOT NULL FOR UPDATE", &[&user_id.as_str()]).await?.context("Connect Trading 212 first.")
}

pub(super) async fn save_pie(user_id: &UserId, client: &Client, id: &str, pie: &super::pies::PieResult) -> anyhow::Result<()> {
    // Only execution evidence is writable after approval. SQL preserves immutable intent.
    let value = serde_json::to_value(pie)?;
    ensure!(client.execute(
        "UPDATE trading_proposals SET pie=pie || jsonb_build_object('status',$3::jsonb->'status','broker_pie_id',$3::jsonb->'broker_pie_id','error',$3::jsonb->'error','submitted_at',$3::jsonb->'submitted_at','observation',$3::jsonb->'observation') WHERE user_id=$1 AND id=$2 AND kind='pie' AND status IN ('executing','completed')",
        &[&user_id.as_str(), &id, &value],
    ).await? == 1, "Pie execution is no longer current.");
    Ok(())
}

