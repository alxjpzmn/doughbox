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
        queries::{listing_change::get_listing_changes, stock_split::get_stock_splits},
    },
    services::instruments::{
        identifiers::get_changed_identifier,
        stock_splits::{get_split_adjusted_price_per_unit, get_split_adjusted_units},
    },
};

use super::taxation::{TaxEventType, TaxRelevantEvent};

#[typeshare]
#[derive(Debug, Clone, Serialize)]
pub enum TradeDirection {
    Buy,
    Sell,
}

pub async fn get_events(
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let client = db_client().await?;

    let (interest_rows, fund_report_rows, dividend_rows, trade_rows, fx_conversion_rows) = try_join!(
        query_interest(&client, &start_date, &end_date),
        query_fund_reports(&client, &start_date, &end_date),
        query_dividends(&client, &start_date, &end_date),
        query_trades(&client, &start_date, &end_date),
        query_fx_conversions(&client, &start_date, &end_date)
    )?;

    let mut events = Vec::new();
    events.extend(process_interest_rows(interest_rows)?);
    events.extend(process_fund_report_rows(fund_report_rows)?);
    events.extend(process_dividend_rows(dividend_rows)?);
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
            "SELECT date, amount, currency, principal, withholding_tax, witholding_tax_currency, amount_eur FROM interest WHERE date >= $1 AND date < $2",
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
            "SELECT date, id, currency FROM fund_reports WHERE date >= $1 AND date < $2",
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
            "SELECT date, amount, currency, isin, withholding_tax, witholding_tax_currency, amount_eur FROM dividends WHERE date >= $1 AND date < $2",
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
            "SELECT date, no_units, avg_price_per_unit, currency_denomination, isin, direction, withholding_tax, witholding_tax_currency, eur_avg_price_per_unit FROM trades WHERE date >= $1 AND date < $2",
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
            "SELECT date, from_amount, to_amount, from_currency, to_currency FROM fx_conversions WHERE date >= $1 AND date < $2",
            &[start_date, end_date],
        )
        .await?)
}

fn process_interest_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        if row.get::<usize, Decimal>(4) != dec!(0.0)
            && row.get::<usize, String>(5) != row.get::<usize, String>(2)
        {
            panic!(
                "Currency for withholding tax doesn't match event currency: {}, for {}",
                row.get::<usize, DateTime<Utc>>(0),
                row.get::<usize, Decimal>(1)
            )
        }

        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: if row.get::<usize, String>(3) == "Cash" {
                TaxEventType::CashInterest
            } else {
                TaxEventType::ShareInterest
            },
            identifier: None,
            units: row.get(1),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: Some(row.get::<usize, Decimal>(1) / row.get::<usize, Decimal>(6)),
            withholding_tax_percent: Some(
                row.get::<usize, Option<Decimal>>(4).unwrap_or(dec!(0.0))
                    / row.get::<usize, Decimal>(1),
            ),
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fund_report_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::DividendAequivalent,
            identifier: Some(row.get::<usize, i32>(1).to_string()),
            units: dec!(1.00),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: None,
            withholding_tax_percent: None,
        };
        events.push(event);
    }
    Ok(events)
}

fn process_dividend_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        if row.get::<usize, Decimal>(4) != dec!(0.0)
            && row.get::<usize, String>(5) != row.get::<usize, String>(2)
        {
            panic!(
                "Currency for withholding tax doesn't match event currency: {}, for {}",
                row.get::<usize, DateTime<Utc>>(0),
                row.get::<usize, String>(3)
            )
        }
        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::Dividend,
            identifier: row.get(3),
            units: row.get(1),
            price_unit: dec!(1.00),
            currency: row.get(2),
            direction: None,
            applied_fx_rate: Some(row.get::<usize, Decimal>(1) / row.get::<usize, Decimal>(6)),
            withholding_tax_percent: Some(
                row.get::<usize, Option<Decimal>>(4).unwrap_or(dec!(0.0))
                    / row.get::<usize, Decimal>(1),
            ),
        };
        events.push(event);
    }
    Ok(events)
}

async fn process_trade_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut stock_split_information = get_stock_splits().await?;
    let listing_changes = get_listing_changes().await?;

    let mut events = Vec::new();
    for row in rows {
        if row.get::<usize, Decimal>(6) != dec!(0.0)
            && row.get::<usize, String>(7) != row.get::<usize, String>(3)
        {
            panic!(
                "Currency for withholding tax doesn't match event currency: {}, for {}",
                row.get::<usize, DateTime<Utc>>(0),
                row.get::<usize, String>(4)
            )
        }

        let split_adjusted_units = get_split_adjusted_units(
            row.get(4),
            row.get(1),
            row.get(0),
            &mut stock_split_information,
        );
        let split_adjusted_price_per_unit = get_split_adjusted_price_per_unit(
            row.get(4),
            row.get(2),
            row.get(0),
            &mut stock_split_information,
        );
        let isin = get_changed_identifier(row.get(4), listing_changes.clone());

        let applied_fx_rate = if row.get::<usize, Decimal>(2) == dec!(0.0) {
            dec!(1)
        } else {
            row.get::<usize, Decimal>(2)
        } / if row.get::<usize, Decimal>(8) == dec!(0.0) {
            dec!(1.0)
        } else {
            row.get::<usize, Decimal>(8)
        };

        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::Trade,
            identifier: Some(isin),
            units: split_adjusted_units,
            price_unit: split_adjusted_price_per_unit,
            currency: row.get(3),
            direction: Some(if row.get::<usize, String>(5) == *"Buy" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(applied_fx_rate),
            withholding_tax_percent: Some(
                row.get::<usize, Option<Decimal>>(6).unwrap_or(dec!(0.00))
                    / (row.get::<usize, Decimal>(1)
                        * if row.get::<usize, Decimal>(2) == dec!(0.0) {
                            dec!(1)
                        } else {
                            row.get::<usize, Decimal>(2)
                        }),
            ),
        };
        events.push(event);
    }
    Ok(events)
}

fn process_fx_conversion_rows(rows: Vec<Row>) -> anyhow::Result<Vec<TaxRelevantEvent>> {
    let mut events = Vec::new();
    for row in rows {
        let event = TaxRelevantEvent {
            date: row.get(0),
            event_type: TaxEventType::FxConversion,
            currency: row.get(3),
            identifier: Some(format!(
                "{}{}",
                row.get::<usize, String>(3),
                row.get::<usize, String>(4)
            )),
            direction: Some(if row.get::<usize, String>(3) == *"EUR" {
                TradeDirection::Buy
            } else {
                TradeDirection::Sell
            }),
            applied_fx_rate: Some(row.get::<usize, Decimal>(2) / row.get::<usize, Decimal>(1)),
            units: row.get(1),
            price_unit: row.get::<usize, Decimal>(2) / row.get::<usize, Decimal>(1),
            withholding_tax_percent: None,
        };
        events.push(event);
    }
    Ok(events)
}