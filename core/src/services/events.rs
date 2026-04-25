use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use tokio::try_join;
use tokio_postgres::{Client, Row};
use typeshare::typeshare;

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
#[derive(Debug, Clone, Serialize)]
pub enum TradeDirection {
    Buy,
    Sell,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum EventType {
    CashInterest,
    ShareInterest,
    Dividend,
    Trade,
    FxConversion,
    DividendAequivalent,
}

#[typeshare]
#[derive(Debug, Clone, Serialize)]
pub struct PortfolioEvent {
    pub date: DateTime<Utc>,
    pub event_type: EventType,
    pub currency: String,
    pub units: Decimal,
    pub price_unit: Decimal,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub direction: Option<TradeDirection>,
    pub applied_fx_rate: Option<Decimal>,
    pub withholding_tax_percent: Option<Decimal>,
    pub total: Decimal,
    pub broker: String,
}

pub async fn get_events(
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
) -> anyhow::Result<Vec<PortfolioEvent>> {
    let client = db_client().await?;

    let (interest_rows, fund_report_rows, dividend_rows, trade_rows, fx_conversion_rows) = try_join!(
        query_interest(&client, &start_date, &end_date),
        query_fund_reports(&client, &start_date, &end_date),
        query_dividends(&client, &start_date, &end_date),
        query_trades(&client, &start_date, &end_date),
        query_fx_conversions(&client, &start_date, &end_date)
    )?;

    let mut events = Vec::new();
    events.extend(process_interest_rows(interest_rows).await?);
    events.extend(process_fund_report_rows(fund_report_rows)?);
    events.extend(process_dividend_rows(dividend_rows).await?);
    events.extend(process_trade_rows(trade_rows).await?);
    events.extend(process_fx_conversion_rows(fx_conversion_rows)?);

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
            "select date, amount, currency, principal, withholding_tax, withholding_tax_currency, amount_eur, broker FROM interest WHERE date >= $1 AND date < $2",
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
            "select date, amount, currency, isin, withholding_tax, withholding_tax_currency, amount_eur, broker FROM dividend WHERE date >= $1 AND date < $2",
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
            "select date, units, avg_price_per_unit, currency, isin, direction, withholding_tax, withholding_tax_currency, eur_avg_price_per_unit, broker FROM trade WHERE date >= $1 AND date < $2",
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
            "select date, from_amount, to_amount, from_currency, to_currency, broker FROM fx_conversion WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

async fn process_interest_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let amount: Decimal = row.get(1);
        let amount_eur: Decimal = row.get(6);
        let withholding_tax = row.get::<usize, Option<Decimal>>(4).unwrap_or(dec!(0.0));
        let event_currency: String = row.get(2);
        let withholding_tax_currency: String = row.get(5);
        let date: DateTime<Utc> = row.get(0);
        
        // Calculate withholding tax percent
        let withholding_tax_percent = if withholding_tax == dec!(0.0) {
            Some(dec!(0.0))
        } else if amount == dec!(0.0) || amount_eur == dec!(0.0) {
            // Can't calculate percentage with zero amount - this indicates bad data
            return Err(anyhow::anyhow!(
                "Cannot calculate withholding tax percent: zero amount for interest on {}. Amount: {}, Amount EUR: {}",
                date, amount, amount_eur
            ));
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
                        date, withholding_tax_currency, 
                        withholding_tax, withholding_tax_currency, 
                        amount_eur, e
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
                        date, withholding_tax_currency, event_currency, 
                        withholding_tax, withholding_tax_currency, 
                        amount_eur, e
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
            date,
            event_type: if row.get::<usize, String>(3) == "Cash" {
                EventType::CashInterest
            } else {
                EventType::ShareInterest
            },
            identifier: None,
            name: None,
            units: amount,
            price_unit: dec!(1.00),
            currency: event_currency,
            direction: None,
            applied_fx_rate,
            withholding_tax_percent,
            total: amount_eur,
            broker: row.get::<usize, String>(7),
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fund_report_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let event = PortfolioEvent {
            date: row.get(0),
            event_type: EventType::DividendAequivalent,
            identifier: Some(row.get::<usize, i32>(1).to_string()),
            name: None,
            units: dec!(1.00),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: None,
            withholding_tax_percent: None,
            total: dec!(1.00),
            broker: "OeKB Fund Report".to_string(),
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
        .map(|row| get_changed_identifier(row.get(3), listing_changes.clone()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let names = batch_get_instrument_names(&isins).await?;
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    for row in rows {
        let amount: Decimal = row.get(1);
        let amount_eur: Decimal = row.get(6);
        let withholding_tax = row.get::<usize, Option<Decimal>>(4).unwrap_or(dec!(0.0));
        let event_currency: String = row.get(2);
        let withholding_tax_currency: String = row.get(5);
        let date: DateTime<Utc> = row.get(0);
        
        // Calculate withholding tax percent
        let withholding_tax_percent = if withholding_tax == dec!(0.0) {
            Some(dec!(0.0))
        } else if amount == dec!(0.0) || amount_eur == dec!(0.0) {
            // Can't calculate percentage with zero amount - this indicates bad data
            return Err(anyhow::anyhow!(
                "Cannot calculate withholding tax percent: zero amount for dividend on {} (ISIN: {}). Amount: {}, Amount EUR: {}",
                date, 
                row.get::<usize, String>(3),
                amount, 
                amount_eur
            ));
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
                        row.get::<usize, String>(3),
                        withholding_tax_currency, 
                        withholding_tax, withholding_tax_currency, 
                        amount_eur, e
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
                        row.get::<usize, String>(3),
                        withholding_tax_currency, 
                        event_currency, 
                        withholding_tax, withholding_tax_currency, 
                        amount_eur, e
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
            date,
            event_type: EventType::Dividend,
            identifier: Some(
                name_map
                    .get(&row.get::<usize, String>(3))
                    .unwrap_or(&&row.get(3))
                    .to_string(),
            ),
            name: None,
            units: amount,
            price_unit: dec!(1.00),
            currency: event_currency,
            direction: None,
            applied_fx_rate,
            withholding_tax_percent,
            total: amount_eur,
            broker: row.get::<usize, String>(7),
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
        .map(|row| get_changed_identifier(row.get(4), listing_changes.clone()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let names = batch_get_instrument_names(&isins).await?;
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    let mut events = Vec::new();
    for row in rows {
        let withholding_tax = row.get::<usize, Option<Decimal>>(6).unwrap_or(dec!(0.0));
        let event_currency: String = row.get(3);
        let withholding_tax_currency: String = row.get(7);
        let date: DateTime<Utc> = row.get(0);
        let units: Decimal = row.get(1);
        let price_per_unit: Decimal = row.get(2);
        let eur_price_per_unit: Decimal = row.get(8);
        let isin: String = row.get(4);
        
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
            return Err(anyhow::anyhow!(
                "Cannot calculate withholding tax percent: zero trade amount for trade on {} (ISIN: {}). \
                Units: {}, Price: {}, EUR Price: {}",
                date, isin, units, price_per_unit, eur_price_per_unit
            ));
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
                        date, isin, withholding_tax_currency, 
                        withholding_tax, withholding_tax_currency, 
                        trade_amount_eur, e
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
                        date, isin, withholding_tax_currency, event_currency, 
                        withholding_tax, withholding_tax_currency, 
                        trade_amount_eur, e
                    ));
                }
            }
        };

        let split_adjusted_units = get_split_adjusted_units(
            row.get(4),
            units,
            date,
            &mut stock_split_information,
        );
        let split_adjusted_price_per_unit = get_split_adjusted_price_per_unit(
            row.get(4),
            price_per_unit,
            date,
            &mut stock_split_information,
        );
        let isin = get_changed_identifier(row.get(4), listing_changes.clone());

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
            date,
            event_type: EventType::Trade,
            identifier: Some(isin.to_string()),
            name: Some(name_map.get(&isin).unwrap_or(&&isin).to_string()),
            units: split_adjusted_units,
            price_unit: split_adjusted_price_per_unit,
            currency: event_currency,
            direction: Some(if row.get::<usize, String>(5) == *"Buy" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(applied_fx_rate),
            withholding_tax_percent,
            total: split_adjusted_units * split_adjusted_price_per_unit,
            broker: row.get::<usize, String>(9),
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fx_conversion_rows(rows: Vec<Row>) -> anyhow::Result<Vec<PortfolioEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let event = PortfolioEvent {
            date: row.get(0),
            event_type: EventType::FxConversion,
            currency: row.get(3),
            identifier: Some(format!(
                "{}{}",
                row.get::<usize, String>(3),
                row.get::<usize, String>(4)
            )),
            name: None,
            direction: Some(if row.get::<usize, String>(3) == *"EUR" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(row.get::<usize, Decimal>(2) / row.get::<usize, Decimal>(1)),
            units: row.get(1),
            price_unit: row.get::<usize, Decimal>(2) / row.get::<usize, Decimal>(1),
            withholding_tax_percent: None,
            total: row.get::<usize, Decimal>(1) * row.get::<usize, Decimal>(2)
                / row.get::<usize, Decimal>(1),
            broker: row.get::<usize, String>(5),
        };
        events.push(event);
    }
    Ok(events)
}
