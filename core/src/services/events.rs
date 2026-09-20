use std::collections::{HashMap, HashSet};

use anyhow::Context;
use chrono::{DateTime, Utc};
use log;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tokio::try_join;
use tokio_postgres::{Client, Row};
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::{
    database::{
        db_client,
        queries::{
            fx_rate::get_exchange_rate, instrument::batch_get_instrument_names,
            listing_change::get_listing_changes, stock_split::get_stock_splits,
        },
    },
    services::instruments::{
        identifiers::get_changed_identifier,
        stock_splits::{get_split_adjusted_price_per_unit, get_split_adjusted_units},
    },
};

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum TradeDirection {
    Buy,
    Sell,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub enum EventType {
    CashInterest,
    ShareInterest,
    PrivateDebtInterest,
    Dividend,
    Trade,
    FxConversion,
    DividendAequivalent,
    Deposit,
    Withdrawal,
    PrincipalAdvance,
    PrincipalRepayment,
    OpeningBalance,
    BalanceReconciliation,
    Valuation,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PortfolioEvent {
    pub event_id: Option<String>,
    pub date: DateTime<Utc>,
    pub event_type: EventType,
    pub currency: String,
    pub units: Decimal,
    pub price_unit: Decimal,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub unit_label: Option<String>,
    pub direction: Option<TradeDirection>,
    pub applied_fx_rate: Option<Decimal>,
    pub withholding_tax_percent: Option<Decimal>,
    pub total: Decimal,
    pub total_currency: String,
    pub broker: String,
    pub tax_supported: bool,
}

pub async fn get_events(
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
) -> anyhow::Result<Vec<PortfolioEvent>> {
    let client = db_client().await?;

    let (
        interest_rows,
        fund_report_rows,
        dividend_rows,
        trade_rows,
        fx_conversion_rows,
        asset_trade_rows,
        asset_transaction_rows,
        asset_snapshot_rows,
        asset_valuation_rows,
    ) = try_join!(
        query_interest(&client, &start_date, &end_date),
        query_fund_reports(&client, &start_date, &end_date),
        query_dividends(&client, &start_date, &end_date),
        query_trades(&client, &start_date, &end_date),
        query_fx_conversions(&client, &start_date, &end_date),
        query_asset_trades(&client, &start_date, &end_date),
        query_asset_transactions(&client, &start_date, &end_date),
        query_asset_snapshots(&client, &start_date, &end_date),
        query_asset_valuations(&client, &start_date, &end_date)
    )?;

    let mut events = Vec::new();
    events.extend(process_interest_rows(interest_rows).await?);
    events.extend(process_fund_report_rows(fund_report_rows)?);
    events.extend(process_dividend_rows(dividend_rows).await?);
    events.extend(process_trade_rows(trade_rows).await?);
    events.extend(process_fx_conversion_rows(fx_conversion_rows)?);
    events.extend(process_asset_trade_rows(asset_trade_rows)?);
    events.extend(process_asset_transaction_rows(asset_transaction_rows)?);
    events.extend(process_asset_snapshot_rows(asset_snapshot_rows)?);
    events.extend(process_asset_valuation_rows(asset_valuation_rows)?);

    events.sort_by(|event_a, event_b| event_a.date.cmp(&event_b.date));

    Ok(events)
}

async fn query_interest(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT i.date, i.amount, i.currency, i.principal, i.withholding_tax, \
             i.withholding_tax_currency, i.amount_eur, i.broker, i.asset_id, \
              i.tax_treatment, a.name, a.unit_label, i.id FROM interest i LEFT JOIN asset a ON a.id = i.asset_id \
             WHERE i.date >= $1 AND i.date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_asset_trades(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT t.id AS event_id, t.date, t.units, t.price_per_unit, t.eur_price_per_unit, t.currency, \
              t.direction, t.broker, a.id AS asset_id, a.name, a.unit_label FROM asset_trade t \
             JOIN asset a ON a.id = t.asset_id WHERE t.date >= $1 AND t.date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_asset_transactions(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT t.id AS event_id, t.date, t.kind, t.amount, t.amount_eur, t.currency, \
              a.id AS asset_id, a.name, a.unit_label \
             FROM asset_transaction t JOIN asset a ON a.id = t.asset_id \
             WHERE t.date >= $1 AND t.date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_asset_snapshots(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT s.id AS event_id, s.date, s.kind, s.balance, s.balance_eur, a.currency, \
              a.id AS asset_id, a.name, a.unit_label \
             FROM asset_balance_snapshot s JOIN asset a ON a.id = s.asset_id \
             WHERE s.date >= $1 AND s.date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_asset_valuations(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "SELECT v.id AS event_id, v.date, v.price_per_unit, v.eur_price_per_unit, \
              v.currency, a.id AS asset_id, a.name, a.unit_label FROM asset_valuation v \
             JOIN asset a ON a.id = v.asset_id WHERE v.source = 'Manual' \
             AND v.date >= $1 AND v.date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_fund_reports(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "select date, id, currency FROM fund_report_oekb WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_dividends(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "select date, amount, currency, isin, withholding_tax, withholding_tax_currency, \
             amount_eur, broker, id FROM dividend WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_trades(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "select date, units, avg_price_per_unit, currency, isin, direction, withholding_tax, \
             withholding_tax_currency, eur_avg_price_per_unit, broker, hash FROM trade \
             WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn query_fx_conversions(
    client: &Client,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) -> anyhow::Result<Vec<Row>> {
    Ok(client
        .query(
            "select date, from_amount, to_amount, from_currency, to_currency, broker, id \
             FROM fx_conversion WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

fn try_get_col<T: for<'a> tokio_postgres::types::FromSql<'a>>(
    row: &Row,
    idx: usize,
    name: &str,
    ctx: &str,
) -> anyhow::Result<T> {
    row.try_get(idx).with_context(|| {
        log::error!("Failed to read column {} ({}) — {}", idx, name, ctx);
        format!("Failed to read column {} ({}) — {}", idx, name, ctx)
    })
}

async fn process_interest_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let date: DateTime<Utc> =
            try_get_col(row, 0, "date", &format!("interest row index {}", idx))?;
        let ctx = format!("interest row index {} date={}", idx, date);

        let amount: Decimal = try_get_col(row, 1, "amount", &ctx)?;
        let amount_eur: Decimal = try_get_col(row, 6, "amount_eur", &ctx)?;
        let withholding_tax: Option<Decimal> = try_get_col(row, 4, "withholding_tax", &ctx)?;
        let event_currency: String = try_get_col(row, 2, "currency", &ctx)?;
        let withholding_tax_currency: Option<String> =
            try_get_col(row, 5, "withholding_tax_currency", &ctx)?;
        let principal: Option<String> = try_get_col(row, 3, "principal", &ctx)?;
        let broker: Option<String> = try_get_col(row, 7, "broker", &ctx)?;
        let asset_id: Option<String> = try_get_col(row, 8, "asset_id", &ctx)?;
        let tax_treatment: String = try_get_col(row, 9, "tax_treatment", &ctx)?;
        let asset_name: Option<String> = try_get_col(row, 10, "name", &ctx)?;
        let unit_label: Option<String> = try_get_col(row, 11, "unit_label", &ctx)?;
        let tax_supported = tax_treatment == "Included";

        let withholding_tax = withholding_tax.unwrap_or(dec!(0.0));
        let withholding_tax_currency =
            withholding_tax_currency.unwrap_or_else(|| event_currency.clone());

        // Calculate withholding tax percent
        let withholding_tax_percent = if !tax_supported {
            None
        } else if withholding_tax == dec!(0.0) {
            Some(dec!(0.0))
        } else if amount == dec!(0.0) || amount_eur == dec!(0.0) {
            // Can't calculate percentage with zero amount — skip withholding tax
            log::warn!(
                "Skipping withholding tax percent for zero-amount interest on {}. Amount: {}, Amount EUR: {}",
                date, amount, amount_eur
            );
            None
        } else if withholding_tax_currency == event_currency {
            // Same currency - simple division
            Some(withholding_tax / amount)
        } else if withholding_tax_currency == "EUR" {
            // Withholding tax already in EUR - use amount_eur
            Some(withholding_tax / amount_eur)
        } else if event_currency == "EUR" {
            // Event is in EUR but withholding tax is in different currency - convert tax to EUR
            let naive_date = date.date_naive();
            match get_exchange_rate(&withholding_tax_currency, "EUR", &naive_date).await {
                Ok(fx_rate) => {
                    let withholding_tax_eur = withholding_tax * fx_rate;
                    Some(withholding_tax_eur / amount_eur)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Cannot calculate withholding tax percent for interest on {}: \
                         cannot convert withholding tax from {} to EUR. \
                         Event currency: EUR, Withholding tax: {} {}, Amount EUR: {}. \
                         Please ensure FX rates are available. Error: {}",
                        date,
                        withholding_tax_currency,
                        withholding_tax,
                        withholding_tax_currency,
                        amount_eur,
                        e
                    ));
                }
            }
        } else {
            // Neither is EUR - convert withholding tax to EUR and calculate
            let naive_date = date.date_naive();
            match get_exchange_rate(&withholding_tax_currency, "EUR", &naive_date).await {
                Ok(fx_rate) => {
                    let withholding_tax_eur = withholding_tax * fx_rate;
                    Some(withholding_tax_eur / amount_eur)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Cannot calculate withholding tax percent for interest on {}: \
                         cannot convert withholding tax from {} to EUR. \
                         Event currency: {}, Withholding tax: {} {}, Amount EUR: {}. \
                         Please ensure FX rates are available. Error: {}",
                        date,
                        withholding_tax_currency,
                        event_currency,
                        withholding_tax,
                        withholding_tax_currency,
                        amount_eur,
                        e
                    ));
                }
            }
        };

        let applied_fx_rate = if amount_eur != dec!(0.0) {
            Some(amount / amount_eur)
        } else {
            None
        };

        let event = PortfolioEvent {
            event_id: Some(try_get_col(row, 12, "id", &ctx)?),
            date,
            event_type: match principal.as_deref() {
                Some("Cash") => EventType::CashInterest,
                Some("PrivateDebt") => EventType::PrivateDebtInterest,
                _ => EventType::ShareInterest,
            },
            identifier: asset_id,
            name: asset_name,
            unit_label,
            units: amount,
            price_unit: dec!(1.00),
            currency: event_currency,
            direction: None,
            applied_fx_rate,
            withholding_tax_percent,
            total: amount_eur,
            total_currency: "EUR".to_string(),
            broker: broker.unwrap_or_else(|| "Manual".to_string()),
            tax_supported,
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fund_report_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let date: DateTime<Utc> =
            try_get_col(row, 0, "date", &format!("fund_report row index {}", idx))?;
        let ctx = format!("fund_report row index {} date={}", idx, date);

        let id: i32 = try_get_col(row, 1, "id", &ctx)?;
        let currency: String = try_get_col(row, 2, "currency", &ctx)?;

        let event = PortfolioEvent {
            event_id: Some(format!("fund-report-{id}")),
            date,
            event_type: EventType::DividendAequivalent,
            identifier: Some(id.to_string()),
            name: None,
            unit_label: None,
            units: dec!(1.00),
            price_unit: dec!(1.00),
            currency: currency.clone(),
            direction: None,
            applied_fx_rate: None,
            withholding_tax_percent: None,
            total: dec!(1.00),
            total_currency: currency.clone(),
            broker: "OeKB Fund Report".to_string(),
            tax_supported: true,
        };
        events.push(event);
    }
    Ok(events)
}

async fn process_dividend_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();

    let listing_changes = get_listing_changes().await?;
    let isins: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let isin: String = try_get_col(row, 3, "isin", &format!("dividend row index {}", idx))?;
            Ok(get_changed_identifier(&isin, listing_changes.clone()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let names = batch_get_instrument_names(&isins).await?;
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    for (idx, row) in rows.iter().enumerate() {
        let date: DateTime<Utc> =
            try_get_col(row, 0, "date", &format!("dividend row index {}", idx))?;
        let ctx = format!("dividend row index {} date={}", idx, date);

        let amount: Decimal = try_get_col(row, 1, "amount", &ctx)?;
        let amount_eur: Decimal = try_get_col(row, 6, "amount_eur", &ctx)?;
        let withholding_tax: Option<Decimal> = try_get_col(row, 4, "withholding_tax", &ctx)?;
        let event_currency: String = try_get_col(row, 2, "currency", &ctx)?;
        let withholding_tax_currency: Option<String> =
            try_get_col(row, 5, "withholding_tax_currency", &ctx)?;
        let isin: String = try_get_col(row, 3, "isin", &ctx)?;
        let broker: String = try_get_col(row, 7, "broker", &ctx)?;

        let withholding_tax = withholding_tax.unwrap_or(dec!(0.0));
        let withholding_tax_currency =
            withholding_tax_currency.unwrap_or_else(|| event_currency.clone());

        // Calculate withholding tax percent
        let withholding_tax_percent = if withholding_tax == dec!(0.0) {
            Some(dec!(0.0))
        } else if amount == dec!(0.0) || amount_eur == dec!(0.0) {
            log::warn!(
                "Skipping withholding tax percent for zero-amount dividend on {} (ISIN: {}). Amount: {}, Amount EUR: {}",
                date,
                isin,
                amount,
                amount_eur
            );
            None
        } else if withholding_tax_currency == event_currency {
            // Same currency - simple division
            Some(withholding_tax / amount)
        } else if withholding_tax_currency == "EUR" {
            // Withholding tax already in EUR - use amount_eur
            Some(withholding_tax / amount_eur)
        } else if event_currency == "EUR" {
            // Event is in EUR but withholding tax is in different currency - convert tax to EUR
            let naive_date = date.date_naive();
            match get_exchange_rate(&withholding_tax_currency, "EUR", &naive_date).await {
                Ok(fx_rate) => {
                    let withholding_tax_eur = withholding_tax * fx_rate;
                    Some(withholding_tax_eur / amount_eur)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Cannot calculate withholding tax percent for dividend on {} (ISIN: {}): \
                         cannot convert withholding tax from {} to EUR. \
                         Event currency: EUR, Withholding tax: {} {}, Amount EUR: {}. \
                         Please ensure FX rates are available. Error: {}",
                        date,
                        isin,
                        withholding_tax_currency,
                        withholding_tax,
                        withholding_tax_currency,
                        amount_eur,
                        e
                    ));
                }
            }
        } else {
            // Neither is EUR - convert withholding tax to EUR and calculate
            let naive_date = date.date_naive();
            match get_exchange_rate(&withholding_tax_currency, "EUR", &naive_date).await {
                Ok(fx_rate) => {
                    let withholding_tax_eur = withholding_tax * fx_rate;
                    Some(withholding_tax_eur / amount_eur)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Cannot calculate withholding tax percent for dividend on {} (ISIN: {}): \
                         cannot convert withholding tax from {} to EUR. \
                         Event currency: {}, Withholding tax: {} {}, Amount EUR: {}. \
                         Please ensure FX rates are available. Error: {}",
                        date,
                        isin,
                        withholding_tax_currency,
                        event_currency,
                        withholding_tax,
                        withholding_tax_currency,
                        amount_eur,
                        e
                    ));
                }
            }
        };

        // Calculate FX rate only if amount_eur is not zero
        let applied_fx_rate = if amount_eur != dec!(0.0) {
            Some(amount / amount_eur)
        } else {
            None
        };

        let event = PortfolioEvent {
            event_id: Some(try_get_col(row, 8, "id", &ctx)?),
            date,
            event_type: EventType::Dividend,
            identifier: Some(name_map.get(&isin).unwrap_or(&&isin).to_string()),
            name: None,
            unit_label: None,
            units: amount,
            price_unit: dec!(1.00),
            currency: event_currency.clone(),
            direction: None,
            applied_fx_rate,
            withholding_tax_percent,
            total: amount_eur,
            total_currency: "EUR".to_string(),
            broker,
            tax_supported: true,
        };
        events.push(event);
    }
    Ok(events)
}

async fn process_trade_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut stock_split_information = get_stock_splits().await?;
    let listing_changes = get_listing_changes().await?;

    let isins: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let isin: String = try_get_col(row, 4, "isin", &format!("trade row index {}", idx))?;
            Ok(get_changed_identifier(&isin, listing_changes.clone()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let names = batch_get_instrument_names(&isins).await?;
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    let mut events = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let date: DateTime<Utc> = try_get_col(row, 0, "date", &format!("trade row index {}", idx))?;
        let ctx = format!("trade row index {} date={}", idx, date);

        let withholding_tax: Option<Decimal> = try_get_col(row, 6, "withholding_tax", &ctx)?;
        let event_currency: String = try_get_col(row, 3, "currency", &ctx)?;
        let withholding_tax_currency: Option<String> =
            try_get_col(row, 7, "withholding_tax_currency", &ctx)?;
        let units: Decimal = try_get_col(row, 1, "units", &ctx)?;
        let price_per_unit: Decimal = try_get_col(row, 2, "avg_price_per_unit", &ctx)?;
        let eur_price_per_unit: Decimal = try_get_col(row, 8, "eur_avg_price_per_unit", &ctx)?;
        let isin_raw: String = try_get_col(row, 4, "isin", &ctx)?;
        let direction: String = try_get_col(row, 5, "direction", &ctx)?;
        let broker: String = try_get_col(row, 9, "broker", &ctx)?;

        let withholding_tax = withholding_tax.unwrap_or(dec!(0.0));
        let withholding_tax_currency =
            withholding_tax_currency.unwrap_or_else(|| event_currency.clone());

        // Calculate trade amounts
        let trade_amount = units
            * if price_per_unit == dec!(0.0) {
                dec!(1)
            } else {
                price_per_unit
            };
        let trade_amount_eur = units
            * if eur_price_per_unit == dec!(0.0) {
                dec!(1)
            } else {
                eur_price_per_unit
            };

        // Calculate withholding tax percent
        let withholding_tax_percent = if withholding_tax == dec!(0.0) {
            Some(dec!(0.0))
        } else if trade_amount == dec!(0.0) || trade_amount_eur == dec!(0.0) {
            log::warn!(
                "Skipping withholding tax percent for zero-amount trade on {} (ISIN: {}). Units: {}, Price: {}, EUR Price: {}",
                date, isin_raw, units, price_per_unit, eur_price_per_unit
            );
            None
        } else if withholding_tax_currency == event_currency {
            // Same currency - simple division
            Some(withholding_tax / trade_amount)
        } else if withholding_tax_currency == "EUR" {
            // Withholding tax already in EUR - use trade_amount_eur
            Some(withholding_tax / trade_amount_eur)
        } else if event_currency == "EUR" {
            // Event is in EUR but withholding tax is in different currency - convert tax to EUR
            let naive_date = date.date_naive();
            match get_exchange_rate(&withholding_tax_currency, "EUR", &naive_date).await {
                Ok(fx_rate) => {
                    let withholding_tax_eur = withholding_tax * fx_rate;
                    Some(withholding_tax_eur / trade_amount_eur)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Cannot calculate withholding tax percent for trade on {} (ISIN: {}): \
                         cannot convert withholding tax from {} to EUR. \
                         Event currency: EUR, Withholding tax: {} {}, Trade amount EUR: {}. \
                         Please ensure FX rates are available. Error: {}",
                        date,
                        isin_raw,
                        withholding_tax_currency,
                        withholding_tax,
                        withholding_tax_currency,
                        trade_amount_eur,
                        e
                    ));
                }
            }
        } else {
            // Neither is EUR - convert withholding tax to EUR and calculate
            let naive_date = date.date_naive();
            match get_exchange_rate(&withholding_tax_currency, "EUR", &naive_date).await {
                Ok(fx_rate) => {
                    let withholding_tax_eur = withholding_tax * fx_rate;
                    Some(withholding_tax_eur / trade_amount_eur)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Cannot calculate withholding tax percent for trade on {} (ISIN: {}): \
                         cannot convert withholding tax from {} to EUR. \
                         Event currency: {}, Withholding tax: {} {}, Trade amount EUR: {}. \
                         Please ensure FX rates are available. Error: {}",
                        date,
                        isin_raw,
                        withholding_tax_currency,
                        event_currency,
                        withholding_tax,
                        withholding_tax_currency,
                        trade_amount_eur,
                        e
                    ));
                }
            }
        };

        let split_adjusted_units =
            get_split_adjusted_units(&isin_raw, units, date, &mut stock_split_information);
        let split_adjusted_price_per_unit = get_split_adjusted_price_per_unit(
            &isin_raw,
            price_per_unit,
            date,
            &mut stock_split_information,
        );
        let isin = get_changed_identifier(&isin_raw, listing_changes.clone());

        let applied_fx_rate = if price_per_unit == dec!(0.0) {
            dec!(1)
        } else {
            price_per_unit
        } / if eur_price_per_unit == dec!(0.0) {
            dec!(1.0)
        } else {
            eur_price_per_unit
        };

        let event = PortfolioEvent {
            event_id: Some(try_get_col(row, 10, "hash", &ctx)?),
            date,
            event_type: EventType::Trade,
            identifier: Some(isin.to_string()),
            name: Some(name_map.get(&isin).unwrap_or(&&isin).to_string()),
            unit_label: None,
            units: split_adjusted_units,
            price_unit: split_adjusted_price_per_unit,
            currency: event_currency.clone(),
            direction: Some(if direction == *"Buy" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(applied_fx_rate),
            withholding_tax_percent,
            total: split_adjusted_units * split_adjusted_price_per_unit,
            total_currency: event_currency.clone(),
            broker,
            tax_supported: true,
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fx_conversion_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let date: DateTime<Utc> =
            try_get_col(row, 0, "date", &format!("fx_conversion row index {}", idx))?;
        let ctx = format!("fx_conversion row index {} date={}", idx, date);

        let from_amount: Decimal = try_get_col(row, 1, "from_amount", &ctx)?;
        let to_amount: Decimal = try_get_col(row, 2, "to_amount", &ctx)?;
        let from_currency: String = try_get_col(row, 3, "from_currency", &ctx)?;
        let to_currency: String = try_get_col(row, 4, "to_currency", &ctx)?;
        let broker: String = try_get_col(row, 5, "broker", &ctx)?;

        let event = PortfolioEvent {
            event_id: Some(try_get_col(row, 6, "id", &ctx)?),
            date,
            event_type: EventType::FxConversion,
            currency: from_currency.clone(),
            identifier: Some(format!("{}{}", from_currency, to_currency)),
            name: None,
            unit_label: None,
            direction: Some(if from_currency == *"EUR" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(to_amount / from_amount),
            units: from_amount,
            price_unit: to_amount / from_amount,
            withholding_tax_percent: None,
            total: from_amount * to_amount / from_amount,
            total_currency: to_currency.clone(),
            broker,
            tax_supported: true,
        };
        events.push(event);
    }
    Ok(events)
}

fn process_asset_trade_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    rows.iter()
        .map(|row| {
            let date: DateTime<Utc> = row.try_get("date")?;
            let units: Decimal = row.try_get("units")?;
            let price_per_unit: Decimal = row.try_get("price_per_unit")?;
            let eur_price_per_unit: Decimal = row.try_get("eur_price_per_unit")?;
            let direction: String = row.try_get("direction")?;
            Ok(PortfolioEvent {
                event_id: Some(row.try_get("event_id")?),
                date,
                event_type: EventType::Trade,
                currency: row.try_get("currency")?,
                units,
                price_unit: price_per_unit,
                identifier: Some(row.try_get("asset_id")?),
                name: Some(row.try_get("name")?),
                unit_label: Some(row.try_get("unit_label")?),
                direction: Some(if direction == "Buy" {
                    TradeDirection::Buy
                } else {
                    TradeDirection::Sell
                }),
                applied_fx_rate: if eur_price_per_unit == dec!(0) {
                    None
                } else {
                    Some(price_per_unit / eur_price_per_unit)
                },
                withholding_tax_percent: None,
                total: units * eur_price_per_unit,
                total_currency: "EUR".to_string(),
                broker: row.try_get("broker")?,
                tax_supported: false,
            })
        })
        .collect()
}

fn process_asset_transaction_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    rows.iter()
        .map(|row| {
            let kind: String = row.try_get("kind")?;
            let event_type = match kind.as_str() {
                "Deposit" => EventType::Deposit,
                "Withdrawal" => EventType::Withdrawal,
                "PrincipalAdvance" => EventType::PrincipalAdvance,
                "PrincipalRepayment" => EventType::PrincipalRepayment,
                _ => return Err(anyhow::anyhow!("Unknown custom asset transaction: {kind}")),
            };
            let amount: Decimal = row.try_get("amount")?;
            let amount_eur: Decimal = row.try_get("amount_eur")?;
            Ok(PortfolioEvent {
                event_id: Some(row.try_get("event_id")?),
                date: row.try_get("date")?,
                event_type,
                currency: row.try_get("currency")?,
                units: amount,
                price_unit: dec!(1),
                identifier: Some(row.try_get("asset_id")?),
                name: Some(row.try_get("name")?),
                unit_label: Some(row.try_get("unit_label")?),
                direction: None,
                applied_fx_rate: if amount_eur > dec!(0) {
                    Some(amount / amount_eur)
                } else {
                    None
                },
                withholding_tax_percent: None,
                total: amount_eur,
                total_currency: "EUR".to_string(),
                broker: "Manual".to_string(),
                tax_supported: false,
            })
        })
        .collect()
}

fn process_asset_snapshot_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    rows.iter()
        .map(|row| {
            let kind: String = row.try_get("kind")?;
            let balance: Decimal = row.try_get("balance")?;
            let balance_eur: Decimal = row.try_get("balance_eur")?;
            Ok(PortfolioEvent {
                event_id: Some(row.try_get("event_id")?),
                date: row.try_get("date")?,
                event_type: if kind == "Opening" {
                    EventType::OpeningBalance
                } else {
                    EventType::BalanceReconciliation
                },
                currency: row.try_get("currency")?,
                units: balance,
                price_unit: dec!(1),
                identifier: Some(row.try_get("asset_id")?),
                name: Some(row.try_get("name")?),
                unit_label: Some(row.try_get("unit_label")?),
                direction: None,
                applied_fx_rate: if balance_eur > dec!(0) {
                    Some(balance / balance_eur)
                } else {
                    None
                },
                withholding_tax_percent: None,
                total: balance_eur,
                total_currency: "EUR".to_string(),
                broker: "Manual".to_string(),
                tax_supported: false,
            })
        })
        .collect()
}

fn process_asset_valuation_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    rows.iter()
        .map(|row| {
            let price_per_unit: Decimal = row.try_get("price_per_unit")?;
            let eur_price_per_unit: Decimal = row.try_get("eur_price_per_unit")?;
            Ok(PortfolioEvent {
                event_id: Some(row.try_get("event_id")?),
                date: row.try_get("date")?,
                event_type: EventType::Valuation,
                currency: row.try_get("currency")?,
                units: dec!(1),
                price_unit: price_per_unit,
                identifier: Some(row.try_get("asset_id")?),
                name: Some(row.try_get("name")?),
                unit_label: Some(row.try_get("unit_label")?),
                direction: None,
                applied_fx_rate: if eur_price_per_unit > dec!(0) {
                    Some(price_per_unit / eur_price_per_unit)
                } else {
                    None
                },
                withholding_tax_percent: None,
                total: eur_price_per_unit,
                total_currency: "EUR".to_string(),
                broker: "Manual".to_string(),
                tax_supported: false,
            })
        })
        .collect()
}
