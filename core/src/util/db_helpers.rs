use std::collections::HashMap;

use crate::services::{
    env::get_env_variable,
    instruments::{
        identifiers::get_changed_identifier,
        stock_splits::{get_split_adjusted_units, StockSplit},
    },
    shared::hash_string,
};

use chrono::prelude::*;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use tabled::Tabled;
use tokio_postgres::{types::ToSql, Client, NoTls, Row};

#[derive(Debug, Serialize)]
pub struct PerformanceSignal {
    pub date: DateTime<Utc>,
    pub total_value: Decimal,
    pub total_invested: Decimal,
}

#[derive(Debug, Tabled, Clone, Serialize)]
pub struct Trade {
    pub broker: String,
    pub date: DateTime<Utc>,
    pub no_units: Decimal,
    pub avg_price_per_unit: Decimal,
    pub eur_avg_price_per_unit: Decimal,
    pub security_type: String,
    pub direction: String,
    pub currency_denomination: String,
    pub isin: String,
    pub date_added: DateTime<Utc>,
    pub fees: Decimal,
    pub withholding_tax: Decimal,
    pub witholding_tax_currency: String,
}

#[derive(Debug, Clone)]
pub struct FxConversion {
    pub date: DateTime<Utc>,
    pub broker: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub from_currency: String,
    pub to_currency: String,
    pub date_added: DateTime<Utc>,
    pub fees: Decimal,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ListingChange {
    pub id: String,
    pub ex_date: DateTime<Utc>,
    pub from_factor: Decimal,
    pub to_factor: Decimal,
    pub from_identifier: String,
    pub to_identifier: String,
}

#[derive(Debug, Tabled, Serialize)]
pub struct Dividend {
    pub isin: String,
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: String,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Decimal,
    pub witholding_tax_currency: String,
}

#[derive(Debug, Tabled, Serialize)]
pub struct InterestPayment {
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: String,
    pub principal: String,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Decimal,
    pub witholding_tax_currency: String,
}

#[derive(Debug, Tabled, Clone)]
pub struct Instrument {
    pub id: String,
    pub last_price_update: DateTime<Utc>,
    pub price: Decimal,
    pub name: String,
}
impl Instrument {
    fn from_row(row: &Row) -> Instrument {
        Instrument {
            id: row.try_get("id").unwrap(),
            last_price_update: row.try_get("last_price_update").unwrap(),
            price: row.try_get("price").unwrap(),
            name: row.try_get("name").unwrap(),
        }
    }
}

#[derive(Debug, Tabled, Clone)]
pub struct FundTaxReport {
    pub id: i32,
    pub date: DateTime<Utc>,
    pub isin: String,
    pub currency: String,
    pub dividend: Decimal,
    pub dividend_aequivalent: Decimal,
    pub intermittent_dividends: Decimal,
    pub withheld_dividend: Decimal,
    pub wac_adjustment: Decimal,
}

impl FundTaxReport {
    fn from_row(row: &Row) -> anyhow::Result<FundTaxReport> {
        Ok(FundTaxReport {
            id: row.try_get("id")?,
            date: row.try_get("date")?,
            isin: row.try_get("isin")?,
            currency: row.try_get("currency")?,
            dividend: row.try_get("dividend")?,
            dividend_aequivalent: row.try_get("dividend_aequivalent")?,
            intermittent_dividends: row.try_get("intermittent_dividend")?,
            withheld_dividend: row.try_get("withheld_dividend")?,
            wac_adjustment: row.try_get("wac_adjustment")?,
        })
    }
}

pub async fn db_client() -> anyhow::Result<Client> {
    let (client, connection) =
        tokio_postgres::connect(get_env_variable("POSTGRES_URL").unwrap().as_str(), NoTls).await?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {}", e);
        }
    });
    Ok(client)
}

pub async fn add_trade_to_db(trade: Trade, id: Option<String>) -> anyhow::Result<()> {
    let client = db_client().await?;

    let hash = if id.is_some() {
        hash_string(format!("{}{}", trade.broker, id.unwrap()).as_str())
    } else {
        // if the broker doesn't share the id of the trade, hash generation falls back to a
        // combination of trade properties
        // removed the trade.date because revolut apparently changed their timestamp handling in
        // the later versions of their statements
        hash_string(
            format!(
                "{}{}{}{}",
                trade.isin, trade.no_units, trade.direction, trade.avg_price_per_unit
            )
            .as_str(),
        )
    };

    client.execute(
        "INSERT INTO trades (hash, date, no_units, avg_price_per_unit, eur_avg_price_per_unit, security_type, direction, currency_denomination, isin, broker, date_added, fees, withholding_tax, witholding_tax_currency) values ($1, $2, $3, $4, $5, $6,$7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT(hash) DO NOTHING",
        &[&hash, &trade.date, &trade.no_units, &trade.avg_price_per_unit, &trade.eur_avg_price_per_unit, &trade.security_type, &trade.direction, &trade.currency_denomination, &trade.isin, &trade.broker, &Utc::now(), &trade.fees, &trade.withholding_tax, &trade.witholding_tax_currency],
        )
    .await?;

    println!("✅ Trade added: {:?}", trade);

    let existing_instrument_entry = get_instrument_by_id(&trade.isin).await?;

    match existing_instrument_entry {
        Some(_) => {
            let last_price_update = existing_instrument_entry.unwrap().last_price_update;

            // Update instrument price if the last update is older than the trade date
            if last_price_update < trade.date {
                let instrument = Instrument {
                    id: trade.isin.clone(),
                    last_price_update: trade.date,
                    price: trade.eur_avg_price_per_unit,
                    name: trade.isin.clone(),
                };

                update_instrument_price(instrument).await?;
            }
        }
        None => {
            // Insert new instrument if it doesn't exist
            let instrument = Instrument {
                id: trade.isin.clone(),
                last_price_update: trade.date,
                price: trade.eur_avg_price_per_unit,
                name: trade.isin.clone(),
            };

            update_instrument_price(instrument).await?;
        }
    }

    Ok(())
}

pub async fn add_performance_signal_to_db(
    performance_signal: PerformanceSignal,
) -> anyhow::Result<()> {
    let client = db_client().await?;

    client
        .execute(
            "INSERT INTO performance (date, total_value, total_invested) values ($1, $2, $3)",
            &[
                &performance_signal.date,
                &performance_signal.total_value,
                &performance_signal.total_invested,
            ],
        )
        .await?;

    Ok(())
}

pub async fn add_fund_report_to_db(report: FundTaxReport) -> anyhow::Result<()> {
    let client = db_client().await?;

    client.execute(
        "INSERT INTO fund_reports (id, date, isin, currency, dividend, dividend_aequivalent, intermittent_dividend, withheld_dividend, wac_adjustment) values ($1, $2, $3, $4, $5, $6,$7, $8, $9) ON CONFLICT(id) DO NOTHING",
        &[&report.id, &report.date, &report.isin, &report.currency, &report.dividend, &report.dividend_aequivalent, &report.intermittent_dividends, &report.withheld_dividend, &report.wac_adjustment])
    .await?;

    Ok(())
}

pub async fn get_performance_signals() -> anyhow::Result<Vec<PerformanceSignal>> {
    let client = db_client().await?;

    let statement: String =
        "SELECT DISTINCT ON (date_trunc('day', date)) date, total_value, total_invested
    FROM performance
    ORDER BY date_trunc('day', date), date DESC;"
            .to_string();

    let rows = client.query(&statement, &[]).await?;

    let mut performance_signals: Vec<PerformanceSignal> = vec![];

    for row in rows {
        let trade = PerformanceSignal {
            date: row.get::<usize, DateTime<Utc>>(0),
            total_value: row.get::<usize, Decimal>(1),
            total_invested: row.get::<usize, Decimal>(2),
        };
        performance_signals.push(trade);
    }

    Ok(performance_signals)
}

pub async fn get_used_currencies() -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;

    let statement: String = "
SELECT DISTINCT currency FROM (
    SELECT to_currency AS currency FROM fx_conversions
    UNION
    SELECT currency_denomination AS currency FROM trades
    UNION
    SELECT currency AS currency FROM interest
) AS all_currencies;
"
    .to_string();

    let rows = client.query(&statement, &[]).await?;

    let mut used_currencies: Vec<String> = vec![];

    for row in rows {
        used_currencies.push(row.get(0));
    }

    Ok(used_currencies)
}

pub async fn get_dividends() -> anyhow::Result<Vec<Dividend>> {
    let client = db_client().await?;

    let statement: String = "SELECT date, isin, amount, broker, currency, amount_eur, withholding_tax, witholding_tax_currency
    FROM dividends
    ORDER BY date DESC;"
        .to_string();

    let rows = client.query(&statement, &[]).await?;

    let mut dividend_entries: Vec<Dividend> = vec![];

    for row in rows {
        let dividend = Dividend {
            date: row.get::<usize, DateTime<Utc>>(0),
            isin: row.get::<usize, String>(1),
            amount: row.get::<usize, Decimal>(2),
            broker: row.get::<usize, String>(3),
            currency: row.get::<usize, String>(4),
            amount_eur: row.get::<usize, Decimal>(5),
            withholding_tax: row.get::<usize, Decimal>(6),
            witholding_tax_currency: row.get::<usize, String>(7),
        };
        dividend_entries.push(dividend);
    }
    Ok(dividend_entries)
}

pub async fn add_dividend_to_db(dividend: Dividend) -> anyhow::Result<()> {
    let client = db_client().await?;

    let hash = hash_string(
        format!(
            "{}{}{}{}",
            dividend.isin, dividend.date, dividend.amount, dividend.broker
        )
        .as_str(),
    );

    client.execute(
            "INSERT INTO dividends (id, isin, date, amount, broker, currency, amount_eur, withholding_tax, witholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &dividend.isin, &dividend.date, &dividend.amount, &dividend.broker, &dividend.currency, &dividend.amount_eur, &dividend.withholding_tax, &dividend.witholding_tax_currency],
        )
    .await?;

    Ok(())
}

pub async fn add_interest_to_db(interest_payment: InterestPayment) -> anyhow::Result<()> {
    let client = db_client().await?;

    let hash = hash_string(
        format!(
            "{}{}{}{}",
            interest_payment.date,
            interest_payment.amount,
            interest_payment.broker,
            interest_payment.principal
        )
        .as_str(),
    );

    // generate id based on date, isin, broker and amount
    client.execute(
            "INSERT INTO interest (id, date, amount, broker, principal, currency, amount_eur, withholding_tax, witholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &interest_payment.date, &interest_payment.amount, &interest_payment.broker, &interest_payment.principal, &interest_payment.currency, &interest_payment.amount_eur, &interest_payment.withholding_tax, &interest_payment.witholding_tax_currency],
        )
    .await?;

    Ok(())
}

pub async fn add_stock_split_to_db(stock_split: StockSplit) -> anyhow::Result<()> {
    let client = db_client().await?;

    client.execute(
            "INSERT INTO stock_splits (id, ex_date, from_factor, to_factor, isin, date_added) values ($1, $2, $3, $4, $5, $6) ON CONFLICT(id) DO NOTHING",
            &[&stock_split.id, &stock_split.ex_date, &stock_split.from_factor, &stock_split.to_factor, &stock_split.isin, &Utc::now()],
        )
    .await?;

    Ok(())
}

pub async fn add_fx_conversion_to_db(fx_conversion: FxConversion) -> anyhow::Result<()> {
    let client = db_client().await?;
    let hash = hash_string(
        format!(
            "{}{}{}{}{}{}",
            fx_conversion.date,
            fx_conversion.broker,
            fx_conversion.from_currency,
            fx_conversion.to_currency,
            fx_conversion.from_amount,
            fx_conversion.to_amount
        )
        .as_str(),
    );

    client.execute(
            "INSERT INTO fx_conversions (id, date, broker, from_amount, to_amount, from_currency, to_currency, date_added, fees) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &fx_conversion.date, &fx_conversion.broker, &fx_conversion.from_amount, &fx_conversion.to_amount, &fx_conversion.from_currency, &fx_conversion.to_currency, &fx_conversion.date_added, &fx_conversion.fees],
        )
    .await?;

    Ok(())
}

pub async fn get_used_isins() -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;

    let statement: String = "SELECT distinct(isin) from trades".to_string();

    let listing_changes = get_listing_changes().await?;

    let rows = client.query(&statement, &[]).await?;

    let mut isins: Vec<String> = vec![];

    for row in rows {
        let isin = get_changed_identifier(&row.get::<usize, String>(0), listing_changes.clone());
        isins.push(isin);
    }

    Ok(isins)
}

pub async fn get_all_trades(count: Option<i32>) -> anyhow::Result<Vec<Trade>> {
    let client = db_client().await?;

    let mut statement: String = "SELECT * from trades order by date desc".to_string();

    statement = match count {
        Some(_) => {
            let stmt_w_count = format!("{} LIMIT {}", statement, count.unwrap());
            stmt_w_count
        }
        None => statement,
    };

    let listing_changes = get_listing_changes().await?;

    let rows = client.query(&statement, &[]).await?;

    let mut trades: Vec<Trade> = vec![];

    for row in rows {
        let trade = Trade {
            broker: row.get::<usize, String>(1),
            date: row.get::<usize, DateTime<Utc>>(2),
            no_units: row.get::<usize, Decimal>(3),
            avg_price_per_unit: row.get::<usize, Decimal>(4),
            eur_avg_price_per_unit: row.get::<usize, Decimal>(5),
            security_type: row.get::<usize, String>(6),
            direction: row.get::<usize, String>(7),
            currency_denomination: row.get::<usize, String>(8),
            isin: get_changed_identifier(&row.get::<usize, String>(9), listing_changes.clone()),
            date_added: row.get::<usize, DateTime<Utc>>(10),
            fees: row.get::<usize, Decimal>(11),
            withholding_tax: row.get::<usize, Decimal>(12),
            witholding_tax_currency: row.get::<usize, String>(13),
        };
        trades.push(trade);
    }

    Ok(trades)
}

pub async fn get_total_sell_value() -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    let result = client
        .query_one(
            "select SUM(t.eur_avg_price_per_unit * t.no_units) from trades t where direction = 'Sell'",
            &[],
        )
        .await?;

    Ok(result.try_get::<usize, Decimal>(0).unwrap_or(dec!(0.0)))
}

pub async fn get_positions_for_isin(
    isin: &str,
    date: Option<DateTime<Utc>>,
) -> anyhow::Result<Decimal> {
    let date = if let Some(date) = date {
        date
    } else {
        Utc::now()
    };

    let positions = get_positions(Some(date), Some(isin)).await?;

    let result = positions.first().unwrap().units;

    Ok(result)
}

#[derive(Debug, Tabled, Serialize)]
pub struct Position {
    pub isin: String,
    pub units: Decimal,
}

pub async fn get_positions(
    date: Option<DateTime<Utc>>,
    isin: Option<&str>,
) -> anyhow::Result<Vec<Position>> {
    let client = db_client().await?;

    let date = date.unwrap_or_else(Utc::now);

    let mut query = String::from("select isin, direction, no_units from trades where date <= $1");
    let mut params: Vec<&(dyn ToSql + Sync)> = vec![&date];

    let for_specific_isin = isin.is_some();
    if for_specific_isin {
        query.push_str(" AND isin = $2");
        let value = &isin;
        params.push(value);
    }

    let rows = client.query(&query, &params).await?;

    let mut stock_split_information = get_stock_splits().await?;
    let listing_changes = get_listing_changes().await?;

    let mut units_map: HashMap<String, Decimal> = HashMap::new();

    for row in rows {
        let isin = get_changed_identifier(&row.get::<usize, String>(0), listing_changes.clone());
        let units = row.get::<usize, Decimal>(2);
        let direction: String = row.get(1);
        let entry = units_map
            .entry(isin.clone())
            .or_insert_with(|| Decimal::from(0));
        let split_adjusted_units =
            get_split_adjusted_units(&isin, units, date, &mut stock_split_information);
        if direction == "Buy" {
            *entry += split_adjusted_units;
        } else if direction == "Sell" {
            *entry -= split_adjusted_units;
        }
    }

    let mut active_units: Vec<Position> = vec![];

    for (isin, units) in units_map {
        let split_adjusted_units =
            get_split_adjusted_units(&isin, units, date, &mut stock_split_information);

        let position = Position {
            isin,
            units: split_adjusted_units,
        };

        if position.units > dec!(0) {
            active_units.push(position);
        }
    }

    Ok(active_units)
}

pub async fn get_total_invested_value() -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    let result = client
        .query_one(
            "select SUM(t.eur_avg_price_per_unit * t.no_units) from trades t where direction = 'Buy'",
            &[],
        )
        .await?;

    Ok(result.try_get::<usize, Decimal>(0).unwrap_or(dec!(0.0)))
}

pub async fn get_fund_report_by_id(id: i32) -> anyhow::Result<FundTaxReport> {
    let client = db_client().await?;

    let row = client
        .query_one("SELECT * FROM fund_reports WHERE id = $1", &[&id])
        .await?;

    FundTaxReport::from_row(&row)
}

pub async fn get_instrument_by_id(id: &str) -> anyhow::Result<Option<Instrument>> {
    let client = db_client().await?;

    let row = client
        .query_opt("SELECT * FROM instruments WHERE id = $1", &[&id])
        .await?;

    match row {
        Some(row) => Ok(Some(Instrument::from_row(&row))),
        None => Ok(None),
    }
}

pub async fn update_instrument_price(instrument: Instrument) -> anyhow::Result<()> {
    let client = db_client().await?;

    let query = "
        INSERT INTO instruments (id, last_price_update, price, name)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (id) DO UPDATE
        SET price = EXCLUDED.price,
            last_price_update = EXCLUDED.last_price_update;
    ";

    client
        .execute(
            query,
            &[
                &instrument.id,
                &instrument.last_price_update,
                &instrument.price,
                &instrument.name,
            ],
        )
        .await?;

    println!("🔄 Instrument added or updated: {:?}", instrument);

    Ok(())
}

pub async fn get_stock_splits() -> anyhow::Result<Vec<StockSplit>> {
    let client = db_client().await?;

    let rows = client.query(r#"select * from stock_splits"#, &[]).await?;

    let mut stock_splits: Vec<StockSplit> = vec![];

    for row in rows {
        let stock_split = StockSplit {
            id: row.get::<usize, String>(0),
            ex_date: row.get::<usize, DateTime<Utc>>(1),
            from_factor: row.get::<usize, Decimal>(2),
            to_factor: row.get::<usize, Decimal>(3),
            isin: row.get::<usize, String>(4),
        };

        stock_splits.push(stock_split);
    }

    Ok(stock_splits)
}

pub async fn get_listing_changes() -> anyhow::Result<Vec<ListingChange>> {
    let client = db_client().await?;

    let rows = client
        .query(r#"select * from listing_changes"#, &[])
        .await?;

    let mut listing_changes: Vec<ListingChange> = vec![];

    for row in rows {
        let listing_change = ListingChange {
            id: row.get::<usize, String>(0),
            ex_date: row.get::<usize, DateTime<Utc>>(1),
            from_factor: row.get::<usize, Decimal>(2),
            to_factor: row.get::<usize, Decimal>(3),
            from_identifier: row.get::<usize, String>(4),
            to_identifier: row.get::<usize, String>(5),
        };

        listing_changes.push(listing_change);
    }

    Ok(listing_changes)
}

#[allow(dead_code)]
pub struct TradeWithHash {
    broker: String,
    date: DateTime<Utc>,
    isin: String,
    avg_price_per_unit: Decimal,
    eur_avg_price_per_unit: Decimal,
    no_units: Decimal,
    direction: String,
    security_type: String,
    currency_denomination: String,
    date_added: DateTime<Utc>,
    fees: Decimal,
    withholding_tax: Decimal,
    witholding_tax_currency: String,
    pub hash: String,
}

pub async fn find_similar_trade(trade: &Trade) -> anyhow::Result<Option<TradeWithHash>> {
    let client = db_client().await?;

    let query = r#"
        SELECT broker, date, isin, avg_price_per_unit, eur_avg_price_per_unit, no_units, 
               direction, security_type, currency_denomination, date_added, fees, 
               withholding_tax, witholding_tax_currency, hash
        FROM trades
        WHERE isin = $1 AND date = $2 AND no_units = $3 AND avg_price_per_unit = $4
    "#;

    let row = client
        .query_opt(
            query,
            &[
                &trade.isin,
                &trade.date,
                &trade.no_units,
                &trade.avg_price_per_unit,
            ],
        )
        .await
        .ok()
        .unwrap();

    if let Some(row) = row {
        let found_trade = TradeWithHash {
            broker: row.get("broker"),
            date: row.get("date"),
            isin: row.get("isin"),
            avg_price_per_unit: row.get("avg_price_per_unit"),
            eur_avg_price_per_unit: row.get("eur_avg_price_per_unit"),
            no_units: row.get("no_units"),
            direction: row.get("direction"),
            security_type: row.get("security_type"),
            currency_denomination: row.get("currency_denomination"),
            date_added: row.get("date_added"),
            fees: row.get("fees"),
            withholding_tax: row.get("withholding_tax"),
            witholding_tax_currency: row.get("witholding_tax_currency"),
            hash: row.get("hash"),
        };

        return Ok(Some(found_trade));
    }
    Ok(None)
}
