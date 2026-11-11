use std::{future::Future, str::FromStr};

use anyhow::{ensure, Context};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use utoipa::ToSchema;

use super::{
    pies::{self, PieAccess},
    provider::{self, BrokerClient},
    store::{self, BrokerReadBucket},
    Instrument, TradeInput,
};
use crate::database::{db_client, models::auth::UserId};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct BrokerCash {
    pub available_to_trade: Option<String>,
    pub in_pies: Option<String>,
    pub reserved_for_orders: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct BrokerPosition {
    pub ticker: String,
    pub name: Option<String>,
    pub isin: Option<String>,
    pub currency: Option<String>,
    pub quantity: Option<String>,
    pub quantity_available_for_trading: Option<String>,
    pub quantity_in_pies: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, ToSchema)]
pub struct BrokerAccountState {
    pub fetched_at: DateTime<Utc>,
    pub id: String,
    pub currency: String,
    pub cash: BrokerCash,
    pub total_value: Option<String>,
    pub positions: Vec<BrokerPosition>,
    pub notice: String,
}

impl BrokerAccountState {
    pub const NOTICE: &'static str = "Live Trading 212 account. available_to_trade excludes pie cash and cash reserved for orders. It is not a Doughbox cash account or imported portfolio balance. Positions are this broker account only; recorded holdings elsewhere do not prove shares or cash are available here. Cross-currency buy funding is not converted.";
}

fn exact_decimal(value: &str) -> anyhow::Result<Decimal> {
    Decimal::from_str_exact(value).or_else(|_| Decimal::from_scientific(value))
        .context("Trading 212 returned a value that is not an exact decimal")
}

/// Recorded Doughbox holdings never satisfy these checks. Missing broker evidence
/// stays missing; it is not treated as zero available cash or zero shares.
pub fn validate_trades(trades: &[TradeInput], instruments: &[Instrument], state: &BrokerAccountState) -> anyhow::Result<()> {
    let mut same_currency = Decimal::ZERO;
    let mut cross_currency = false;
    for trade in trades {
        let instrument = instruments.iter().find(|item| item.ticker == trade.ticker)
            .with_context(|| format!("Unknown Trading 212 ticker: {}.", trade.ticker))?;
        let quantity = exact_decimal(&trade.quantity)?;
        if trade.side == "sell" {
            let available = state.positions.iter().find(|item| item.ticker == trade.ticker)
                .and_then(|item| item.quantity_available_for_trading.as_deref())
                .with_context(|| format!("Trading 212 did not report shares available to sell for {}. Recorded Doughbox holdings and shares inside Pies are not sellable availability.", trade.ticker))?;
            ensure!(exact_decimal(available)? >= quantity, "Trading 212 reports {available} {} shares available to trade; the requested sell is {quantity}.", trade.ticker);
        } else if instrument.currency == state.currency {
            same_currency = same_currency.checked_add(quantity.checked_mul(exact_decimal(&trade.limit_price)?).context("Buy notional overflow")?)
                .context("Buy notional overflow")?;
        } else {
            cross_currency = true;
        }
    }
    if same_currency > Decimal::ZERO || cross_currency {
        let raw = state.cash.available_to_trade.as_deref().context(
            "Trading 212 did not report funds available to trade. Doughbox cash accounts and pie cash are not broker buying power.",
        )?;
        let available = exact_decimal(raw)?;
        ensure!(available > Decimal::ZERO, "Trading 212 reports no funds available to trade. Pie cash and Doughbox cash accounts are not included.");
        ensure!(available >= same_currency, "Trading 212 available cash is {available} {}; same-currency buy notionals are {same_currency}. Pie cash is not included.", state.currency);
    }
    Ok(())
}

pub async fn load(user_id: &UserId, access: PieAccess<'_>, wait: bool) -> anyhow::Result<BrokerAccountState> {
    super::check_setup()?;
    let mut client = db_client().await?;
    let (generation, environment, encrypted) = {
        let tx = client.transaction().await?;
        let row = match access {
            PieAccess::Browser => store::browser_connection(user_id, &tx).await?,
            PieAccess::Agent(hash) => store::agent_connection(user_id, &tx, hash).await?,
        };
        let snapshot = (row.get::<_, String>("generation"), row.get::<_, String>("environment"), row.get::<_, String>("credentials"));
        tx.commit().await?;
        snapshot
    };
    let broker = BrokerClient::new(&environment, provider::decrypt_credentials(&encrypted)?)?;
    let summary = paced(user_id, &client, &generation, BrokerReadBucket::Account, wait, broker.account_summary_value()).await?;
    let mut state = decode_summary(&summary, Utc::now())?;
    let positions = paced(user_id, &client, &generation, BrokerReadBucket::Positions, wait, broker.positions_value()).await?;
    state.positions = decode_positions(&positions)?;
    let tx = client.transaction().await?;
    let row = match access {
        PieAccess::Browser => store::browser_connection(user_id, &tx).await?,
        PieAccess::Agent(hash) => store::agent_connection(user_id, &tx, hash).await?,
    };
    ensure!(row.get::<_, String>("generation") == generation, "Trading connection changed. Read the broker account again.");
    tx.commit().await?;
    Ok(state)
}

async fn paced(user_id: &UserId, client: &tokio_postgres::Client, generation: &str, bucket: BrokerReadBucket, wait: bool, request: impl Future<Output = anyhow::Result<Value>>) -> anyhow::Result<Value> {
    read_bucket(user_id, client, generation, bucket, wait).await?;
    match request.await {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(rate) = error.downcast_ref::<pies::PieRateLimited>() {
                store::extend_broker_read(user_id, client, generation, bucket, rate.retry_after_seconds).await?;
            }
            Err(error)
        }
    }
}

async fn read_bucket(user_id: &UserId, client: &tokio_postgres::Client, generation: &str, bucket: BrokerReadBucket, wait: bool) -> anyhow::Result<()> {
    loop {
        match store::claim_broker_read(user_id, client, generation, bucket).await {
            Ok(()) => return Ok(()),
            Err(error) if wait => {
                if let Some(rate) = error.downcast_ref::<pies::PieRateLimited>() {
                    tokio::time::sleep(std::time::Duration::from_secs(rate.retry_after_seconds)).await;
                    continue;
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }
}

fn number(value: Option<&Value>) -> anyhow::Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(raw)) => Ok(Some(raw.to_string())),
        Some(Value::String(raw)) if Number::from_str(raw).is_ok() => Ok(Some(raw.clone())),
        _ => anyhow::bail!("Trading 212 returned a non-decimal account value"),
    }
}
fn text(value: Option<&Value>) -> anyhow::Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) if !raw.trim().is_empty() => Ok(Some(raw.clone())),
        Some(Value::String(_)) => Ok(None),
        _ => anyhow::bail!("Trading 212 returned an invalid account string"),
    }
}
fn decode_summary(value: &Value, fetched_at: DateTime<Utc>) -> anyhow::Result<BrokerAccountState> {
    let object = value.as_object().context("Trading 212 account summary was not an object")?;
    let id = match object.get("id") {
        Some(Value::String(raw)) if !raw.trim().is_empty() => raw.clone(),
        Some(Value::Number(raw)) => raw.to_string(),
        _ => anyhow::bail!("Trading 212 returned an invalid account identifier"),
    };
    let cash = object.get("cash").and_then(Value::as_object);
    Ok(BrokerAccountState {
        fetched_at, id,
        currency: text(object.get("currency"))?.context("Trading 212 returned an invalid account currency")?,
        cash: BrokerCash {
            available_to_trade: number(cash.and_then(|cash| cash.get("availableToTrade")))?,
            in_pies: number(cash.and_then(|cash| cash.get("inPies")))?,
            reserved_for_orders: number(cash.and_then(|cash| cash.get("reservedForOrders")))?,
        },
        total_value: number(object.get("totalValue"))?,
        positions: Vec::new(),
        notice: BrokerAccountState::NOTICE.into(),
    })
}
fn decode_positions(value: &Value) -> anyhow::Result<Vec<BrokerPosition>> {
    value.as_array().context("Trading 212 positions response was not an array")?.iter().map(|row| {
        let row = row.as_object().context("Trading 212 position was not an object")?;
        let instrument = row.get("instrument").and_then(Value::as_object);
        Ok(BrokerPosition {
            ticker: text(instrument.and_then(|item| item.get("ticker")))?.context("Trading 212 position has no ticker")?,
            name: text(instrument.and_then(|item| item.get("name")))?,
            isin: text(instrument.and_then(|item| item.get("isin")))?,
            currency: text(instrument.and_then(|item| item.get("currency")))?,
            quantity: number(row.get("quantity"))?,
            quantity_available_for_trading: number(row.get("quantityAvailableForTrading"))?,
            quantity_in_pies: number(row.get("quantityInPies"))?,
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::trading::TradeInput;

    fn trade(side: &str, ticker: &str, quantity: &str) -> TradeInput {
        TradeInput { ticker: ticker.into(), side: side.into(), quantity: quantity.into(), limit_price: "10".into(), time_validity: "DAY".into() }
    }
    fn state(cash: Option<&str>, positions: Vec<BrokerPosition>) -> BrokerAccountState {
        BrokerAccountState {
            fetched_at: Utc::now(), id: "1".into(), currency: "EUR".into(),
            cash: BrokerCash { available_to_trade: cash.map(str::to_string), in_pies: None, reserved_for_orders: None },
            total_value: None, positions, notice: BrokerAccountState::NOTICE.into(),
        }
    }
    fn instrument(ticker: &str, currency: &str) -> Instrument {
        Instrument { ticker: ticker.into(), name: ticker.into(), currency: currency.into(), isin: "X".into() }
    }

    #[test]
    fn sells_require_broker_available_quantity_not_recorded_holdings() {
        let instruments = vec![instrument("AAPL_US_EQ", "USD")];
        let missing = state(Some("100"), vec![]);
        assert!(validate_trades(&[trade("sell", "AAPL_US_EQ", "1")], &instruments, &missing).unwrap_err().to_string().contains("did not report shares"));
        let short = state(Some("100"), vec![BrokerPosition { ticker: "AAPL_US_EQ".into(), name: None, isin: None, currency: Some("USD".into()), quantity: Some("5".into()), quantity_available_for_trading: Some("0.5".into()), quantity_in_pies: Some("4.5".into()) }]);
        assert!(validate_trades(&[trade("sell", "AAPL_US_EQ", "1")], &instruments, &short).is_err());
        assert!(validate_trades(&[trade("sell", "AAPL_US_EQ", "0.5")], &instruments, &short).is_ok());
    }

    #[test]
    fn buys_use_available_to_trade_and_reject_missing_or_zero_cash() {
        let instruments = vec![instrument("SAP_DE_EQ", "EUR"), instrument("AAPL_US_EQ", "USD")];
        let funded = state(Some("100.00"), vec![]);
        assert!(validate_trades(&[trade("buy", "SAP_DE_EQ", "10")], &instruments, &funded).is_ok());
        assert!(validate_trades(&[trade("buy", "SAP_DE_EQ", "10.01")], &instruments, &funded).is_err());
        assert!(validate_trades(&[trade("buy", "AAPL_US_EQ", "1")], &instruments, &state(None, vec![])).unwrap_err().to_string().contains("did not report funds"));
        assert!(validate_trades(&[trade("buy", "AAPL_US_EQ", "1")], &instruments, &state(Some("0"), vec![])).is_err());
    }
}
