mod approval;
mod catalog;
mod quotes;
pub mod mcp;
pub mod provider;
mod store;

use anyhow::{ensure, Context};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_postgres::Row;
use utoipa::ToSchema;

use crate::{database::db_client, services::shared::env::get_env_variable};
use provider::{BrokerClient, Credentials, Instrument, SubmitError};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TradeInput {
    /// Exact Trading 212 ticker from search_instruments, not an exchange symbol or ISIN.
    pub ticker: String,
    pub side: String,
    /// Positive exact decimal string, never signed. Fractional shares are subject to
    /// Trading 212's instrument/position quantity precision; do not assume four decimals.
    /// Approved quantities are never automatically rounded.
    pub quantity: String,
    /// Positive decimal string in the instrument's native trading currency (including GBX where applicable).
    pub limit_price: String,
    #[serde(default = "day")]
    pub time_validity: String,
}
fn day() -> String {
    "DAY".into()
}

#[derive(Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubmitInput {
    /// Stable key for this proposal. Reuse only for identical requests; retries never create duplicates.
    pub client_request_id: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    /// One trade or up to 50 trades reviewed together. Execution is sequential, not atomic.
    pub trades: Vec<TradeInput>,
}

#[derive(Clone, Deserialize, Serialize, ToSchema)]
pub struct TradeResult {
    #[serde(flatten)]
    pub trade: TradeInput,
    pub name: String,
    pub currency: String,
    pub status: String,
    pub broker_order_id: Option<String>,
    pub error: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct Proposal {
    pub id: String,
    pub client_request_id: String,
    pub title: String,
    pub note: String,
    pub environment: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub trades: Vec<TradeResult>,
}
impl Proposal {
    fn from_row(row: &Row) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get("id"),
            client_request_id: row.get("client_request_id"),
            title: row.get("title"),
            note: row.get("note"),
            environment: row.get("environment"),
            status: row.get("status"),
            created_at: row.get("created_at"),
            trades: serde_json::from_value(row.get("trades"))?,
        })
    }
}

pub struct SubmitOutcome {
    pub proposal: Proposal,
    pub created: bool,
}
#[derive(Serialize, ToSchema)]
pub struct TradingState {
    configured: bool,
    environment: Option<String>,
    account: Option<Value>,
    agent_token_set: bool,
    mcp_path: &'static str,
    setup_error: Option<String>,
    proposals: Vec<Proposal>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectInput {
    pub api_key: String,
    pub api_secret: String,
    pub environment: String,
}

pub fn check_setup() -> anyhow::Result<()> {
    ensure!(
        get_env_variable("PASSWORD").is_some_and(|v| !v.trim().is_empty()),
        "Set a non-empty PASSWORD on the server before enabling trading."
    );
    provider::check_master_key()
}

pub async fn state() -> anyhow::Result<TradingState> {
    store::recover_interrupted().await?;
    let client = db_client().await?;
    let row = client.query_one("SELECT * FROM trading_connection WHERE singleton", &[]).await?;
    let rows = client.query("SELECT * FROM trading_proposals WHERE status IN ('pending', 'executing') OR id IN (SELECT id FROM trading_proposals WHERE status IN ('completed', 'discarded') ORDER BY created_at DESC LIMIT 50) ORDER BY created_at DESC", &[]).await?;
    let mut proposals = rows.iter().map(Proposal::from_row).collect::<anyhow::Result<Vec<_>>>()?;
    if let (Some(environment), Some(credentials)) = (row.get::<_, Option<String>>("environment"), row.get::<_, Option<String>>("credentials")) {
        if let Err(error) = reconcile_orders(&client, &environment, &credentials, &mut proposals).await {
            tracing::warn!(%error, "Trading 212 order status poll failed");
        }
    }
    Ok(TradingState {
        configured: row.get::<_, Option<String>>("credentials").is_some(),
        environment: row.get("environment"),
        account: row.get("account"),
        agent_token_set: row.get::<_, Option<String>>("agent_token_hash").is_some(),
        mcp_path: "/api/trading/mcp",
        setup_error: check_setup().err().map(|e| e.to_string()),
        proposals,
    })
}

fn tracked(status: &str) -> bool {
    matches!(status, "submitted" | "open" | "partial")
}

async fn reconcile_orders(client: &tokio_postgres::Client, environment: &str, credentials: &str, proposals: &mut [Proposal]) -> anyhow::Result<()> {
    let watching = proposals.iter().any(|proposal| proposal.status != "executing" && proposal.trades.iter().any(|trade| tracked(&trade.status) && trade.broker_order_id.is_some()));
    if !watching || !store::claim_poll(client, "last_pending_poll_at", "6 seconds").await? {
        return Ok(());
    }
    let broker = BrokerClient::new(environment, provider::decrypt_credentials(credentials)?)?;
    let pending = broker.pending_order_statuses().await?;
    let pending_ids: std::collections::HashSet<_> = pending.iter().map(|(id, _)| id.clone()).collect();
    let pending_status: std::collections::HashMap<_, _> = pending.into_iter().collect();
    let mut missing = false;
    for proposal in proposals.iter_mut() {
        if proposal.status == "executing" { continue; }
        let mut updates = std::collections::HashMap::new();
        for trade in &proposal.trades {
            let Some(id) = trade.broker_order_id.as_ref() else { continue };
            if !tracked(&trade.status) { continue; }
            if let Some(status) = pending_status.get(id) {
                if trade.status != *status { updates.insert(id.clone(), status.clone()); }
            } else {
                missing = true;
            }
        }
        if !updates.is_empty() { *proposal = store::reconcile_order_statuses(client, &proposal.id, &updates).await?; }
    }
    if !missing || !store::claim_poll(client, "last_history_poll_at", "6 seconds").await? {
        return Ok(());
    }
    let history: std::collections::HashMap<_, _> = broker.historical_order_statuses().await?.into_iter().collect();
    for proposal in proposals.iter_mut() {
        if proposal.status == "executing" { continue; }
        let mut updates = std::collections::HashMap::new();
        for trade in &proposal.trades {
            let Some(id) = trade.broker_order_id.as_ref() else { continue };
            if !tracked(&trade.status) || pending_ids.contains(id) { continue; }
            if let Some(status) = history.get(id) {
                if trade.status != *status { updates.insert(id.clone(), status.clone()); }
            }
        }
        if !updates.is_empty() { *proposal = store::reconcile_order_statuses(client, &proposal.id, &updates).await?; }
    }
    Ok(())
}

pub async fn connect(input: ConnectInput) -> anyhow::Result<TradingState> {
    check_setup()?;
    ensure!(
        matches!(input.environment.as_str(), "demo" | "live"),
        "Choose demo or live."
    );
    ensure!(
        !input.api_key.trim().is_empty() && !input.api_secret.trim().is_empty(),
        "API key and secret are required."
    );
    ensure!(
        input.api_key.len() <= 4096 && input.api_secret.len() <= 4096,
        "Credentials are too long."
    );
    let credentials = Credentials {
        api_key: input.api_key,
        api_secret: input.api_secret,
    };
    let encrypted = provider::encrypt_credentials(&credentials)?;
    let broker = BrokerClient::new(&input.environment, credentials)?;
    let account = broker.account().await?;
    let instruments = broker.instruments().await?;
    ensure!(
        !instruments.is_empty(),
        "Trading 212 returned no supported instruments."
    );
    store::replace_connection(Some((
        &input.environment,
        &encrypted,
        &account,
        &serde_json::to_value(instruments)?,
    )))
    .await?;
    state().await
}
pub async fn disconnect() -> anyhow::Result<TradingState> {
    store::replace_connection(None).await?;
    state().await
}

pub async fn rotate_token() -> anyhow::Result<String> {
    check_setup()?;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("doughbox_trading_{}", blake3::Hash::from(bytes).to_hex());
    let hash = token_hash(&token);
    let client = db_client().await?;
    ensure!(client.execute("UPDATE trading_connection SET agent_token_hash=$1 WHERE singleton AND credentials IS NOT NULL", &[&hash]).await? == 1, "Connect Trading 212 first.");
    Ok(token)
}
pub fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}
pub async fn authenticate(hash: &str) -> anyhow::Result<()> {
    check_setup()?;
    let client = db_client().await?;
    ensure!(client.query_opt("SELECT 1 FROM trading_connection WHERE singleton AND credentials IS NOT NULL AND agent_token_hash=$1", &[&hash]).await?.is_some(), "Invalid or revoked trading token.");
    Ok(())
}

fn positive_decimal(value: &str) -> anyhow::Result<Decimal> {
    // Strict plain decimals, no exponent/whitespace or silent rounding beyond Decimal precision.
    ensure!(
        value.len() <= 60
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit() || b == b'.'),
        "Use positive plain decimal strings."
    );
    let decimal = Decimal::from_str_exact(value).context("Decimal exceeds supported precision.")?;
    ensure!(
        decimal > Decimal::ZERO,
        "Quantity and limit price must be positive."
    );
    Ok(decimal)
}
fn validate(input: &SubmitInput) -> anyhow::Result<()> {
    ensure!(
        !input.client_request_id.trim().is_empty() && input.client_request_id.len() <= 200,
        "client_request_id must contain 1–200 characters."
    );
    ensure!(
        !input.title.trim().is_empty() && input.title.len() <= 200,
        "Title must contain 1–200 characters."
    );
    ensure!(
        input.note.len() <= 4000,
        "Note is limited to 4000 characters."
    );
    ensure!(
        (1..=50).contains(&input.trades.len()),
        "Submit between 1 and 50 trades."
    );
    for trade in &input.trades {
        ensure!(
            matches!(trade.side.as_str(), "buy" | "sell"),
            "Side must be buy or sell."
        );
        ensure!(
            matches!(trade.time_validity.as_str(), "DAY" | "GOOD_TILL_CANCEL"),
            "time_validity must be DAY or GOOD_TILL_CANCEL."
        );
        positive_decimal(&trade.quantity)?;
        positive_decimal(&trade.limit_price)?;
    }
    Ok(())
}

pub async fn submit(hash: &str, input: SubmitInput) -> anyhow::Result<SubmitOutcome> {
    check_setup()?;
    validate(&input)?;
    let catalog = catalog::load(hash).await?;
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    let connection = store::agent_connection(&tx, hash).await?;
    let generation: String = connection.get("generation");
    ensure!(
        generation == catalog.generation,
        "Connection changed. Submit a new proposal."
    );
    let request = serde_json::to_value(&input)?;
    if let Some(row) = tx
        .query_opt(
            "SELECT * FROM trading_proposals WHERE generation=$1 AND client_request_id=$2",
            &[&generation, &input.client_request_id],
        )
        .await?
    {
        ensure!(
            row.get::<_, Value>("request") == request,
            "Idempotency conflict: client_request_id already has different trades or text."
        );
        return Ok(SubmitOutcome {
            proposal: Proposal::from_row(&row)?,
            created: false,
        });
    }
    let instruments = catalog.instruments;
    let mut trades = Vec::with_capacity(input.trades.len());
    for trade in &input.trades {
        let instrument = instruments
            .iter()
            .find(|i| i.ticker == trade.ticker)
            .with_context(|| {
                format!(
                    "Unknown Trading 212 ticker: {}. Use search_instruments.",
                    trade.ticker
                )
            })?;
        trades.push(TradeResult {
            trade: trade.clone(),
            name: instrument.name.clone(),
            currency: instrument.currency.clone(),
            status: "pending".into(),
            broker_order_id: None,
            error: None,
        });
    }
    let id = uuid::Uuid::new_v4().to_string();
    let environment: String = connection.get("environment");
    let row = tx.query_one("INSERT INTO trading_proposals (id,generation,client_request_id,request,title,note,environment,trades) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *", &[&id,&generation,&input.client_request_id,&request,&input.title,&input.note,&environment,&serde_json::to_value(trades)?]).await?;
    tx.commit().await?;
    Ok(SubmitOutcome {
        proposal: Proposal::from_row(&row)?,
        created: true,
    })
}
pub async fn search(hash: &str, query: &str) -> anyhow::Result<Vec<Instrument>> {
    ensure!(
        (1..=100).contains(&query.trim().len()),
        "Search query must contain 1–100 characters."
    );
    let instruments = catalog::load(hash).await?.instruments;
    let query = query.to_lowercase();
    Ok(instruments
        .into_iter()
        .filter(|i| {
            i.ticker.to_lowercase().contains(&query)
                || i.name.to_lowercase().contains(&query)
                || i.isin.to_lowercase().contains(&query)
        })
        .take(50)
        .collect())
}
pub async fn get_proposal(hash: &str, id: &str) -> anyhow::Result<Proposal> {
    let mut client = db_client().await?;
    let tx = client.transaction().await?;
    let connection = store::agent_connection(&tx, hash).await?;
    let generation: String = connection.get("generation");
    let row = tx
        .query_opt(
            "SELECT * FROM trading_proposals WHERE id=$1 AND generation=$2",
            &[&id, &generation],
        )
        .await?
        .context("Proposal not found.")?;
    Proposal::from_row(&row)
}

pub async fn discard(id: &str) -> anyhow::Result<Proposal> {
    store::discard(id).await
}
pub async fn approve(id: String) -> anyhow::Result<Proposal> {
    check_setup()?;
    // The execution continues if the browser disconnects. Durable per-leg markers prevent replay.
    tokio::spawn(async move { execute(&id).await }).await?
}
async fn execute(id: &str) -> anyhow::Result<Proposal> {
    let mut client = db_client().await?;
    ensure!(
        store::execution_lock(&client).await?,
        "Another proposal is executing. Wait for it to finish."
    );
    store::recover_on(&client).await?;
    let tx = client.transaction().await?;
    let connection = tx
        .query_one(
            "SELECT * FROM trading_connection WHERE singleton FOR UPDATE",
            &[],
        )
        .await?;
    let generation: String = connection.get("generation");
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
        "Only pending proposals can be approved; execution is never retried."
    );
    ensure!(
        row.get::<_, String>("generation") == generation,
        "Connection changed. Submit a new proposal."
    );
    let encrypted: Option<String> = connection.get("credentials");
    let credentials =
        provider::decrypt_credentials(&encrypted.context("Connect Trading 212 first.")?)?;
    let broker = BrokerClient::new(&proposal.environment, credentials)?;
    tx.execute(
        "UPDATE trading_proposals SET status='executing' WHERE id=$1",
        &[&id],
    )
    .await?;
    tx.commit().await?;
    proposal.status = "executing".into();
    for index in 0..proposal.trades.len() {
        store::pace_submission(&client).await?;
        proposal.trades[index].status = "submitting".into();
        store::save_trades(&client, id, &proposal.trades).await?;
        let trade = &proposal.trades[index].trade;
        let outcome = broker
            .submit_limit(
                &trade.ticker,
                &trade.side,
                positive_decimal(&trade.quantity)?,
                positive_decimal(&trade.limit_price)?,
                &trade.time_validity,
            )
            .await;
        store::mark_submission(&client).await?;
        let stop = match outcome {
            Ok(broker_id) => {
                proposal.trades[index].status = "submitted".into();
                proposal.trades[index].broker_order_id = Some(broker_id);
                false
            }
            Err(error) => {
                let (status, message) = match error {
                    SubmitError::Rejected(message) => ("failed", message),
                    SubmitError::Unknown(message) => ("unknown", message),
                };
                proposal.trades[index].status = status.into();
                proposal.trades[index].error = Some(message);
                true
            }
        };
        if stop {
            for remaining in &mut proposal.trades[index + 1..] {
                remaining.status = "discarded".into();
                remaining.error =
                    Some("Not sent: an earlier trade failed or has an uncertain outcome.".into());
            }
        }
        store::save_trades(&client, id, &proposal.trades).await?;
        if stop {
            break;
        }
    }
    client
        .execute(
            "UPDATE trading_proposals SET status='completed' WHERE id=$1",
            &[&id],
        )
        .await?;
    proposal.status = "completed".into();
    Ok(proposal)
}

#[cfg(test)]
mod tests {
    use super::*;
    mod reconciliation;
    #[test]
    fn approved_amounts_cannot_round_or_change_sign() {
        for invalid in [
            "0",
            "-1",
            "1e2",
            " 1",
            "1.00000000000000000000000000001",
            "1..2",
            "NaN",
        ] {
            assert!(positive_decimal(invalid).is_err(), "accepted {invalid}");
        }
        assert_eq!(
            positive_decimal("0.00000001").unwrap(),
            Decimal::from_str_exact("0.00000001").unwrap()
        );
    }
}
