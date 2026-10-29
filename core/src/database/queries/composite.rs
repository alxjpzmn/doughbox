use chrono::{DateTime, Utc};
use tokio_postgres::Client;

use crate::{
    database::{
        db_client,
        models::{
            instrument::Instrument, investment_activity::MonthlyInvestmentActivity, trade::Trade,
        },
        queries::instrument::update_instrument_price,
    },
    services::{instruments::identifiers::get_changed_identifier, shared::util::hash_string},
};

use super::listing_change::get_listing_changes;

#[cfg(test)]
mod tests;

pub async fn get_monthly_investment_activity(
    client: &Client,
    start_date: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<MonthlyInvestmentActivity>> {
    let rows = client
        .query(
            "SELECT to_char(date_trunc('month', date AT TIME ZONE 'UTC'), 'YYYY-MM') AS month,
                    ROUND(SUM(net_purchases_eur), 2) AS net_purchases_eur,
                    ROUND(SUM(net_cash_added_eur), 2) AS net_cash_added_eur,
                    ROUND(SUM(dividends_interest_eur), 2) AS dividends_interest_eur
             FROM (
                 SELECT date,
                        CASE direction
                            WHEN 'Buy' THEN eur_avg_price_per_unit * units
                            ELSE -(eur_avg_price_per_unit * units)
                        END AS net_purchases_eur,
                        0::numeric AS net_cash_added_eur,
                        0::numeric AS dividends_interest_eur
                 FROM trade
                 WHERE direction IN ('Buy', 'Sell')
                 UNION ALL
                 SELECT date,
                        CASE direction
                            WHEN 'Buy' THEN eur_price_per_unit * units
                            ELSE -(eur_price_per_unit * units)
                        END,
                        0::numeric,
                        0::numeric
                 FROM asset_trade
                 UNION ALL
                 SELECT tx.date,
                        0::numeric,
                        CASE tx.kind
                            WHEN 'Deposit' THEN tx.amount_eur
                            ELSE -tx.amount_eur
                        END,
                        0::numeric
                 FROM asset_transaction AS tx
                 JOIN asset ON asset.id = tx.asset_id
                 WHERE asset.asset_class = 'CashAccount'
                   AND tx.kind IN ('Deposit', 'Withdrawal')
                 UNION ALL
                 SELECT date,
                        0::numeric,
                        0::numeric,
                        amount_eur
                 FROM dividend
                 UNION ALL
                 SELECT date,
                        0::numeric,
                        0::numeric,
                        amount_eur
                 FROM interest
             ) AS activity
             WHERE ($1::timestamptz IS NULL OR date >= $1)
             GROUP BY 1
             ORDER BY 1",
            &[&start_date],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|row| MonthlyInvestmentActivity {
            month: row.get("month"),
            net_purchases_eur: row.get("net_purchases_eur"),
            net_cash_added_eur: row.get("net_cash_added_eur"),
            dividends_interest_eur: row.get("dividends_interest_eur"),
        })
        .collect())
}

pub async fn get_used_currencies() -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;

    let statement: String = "
        select distinct currency from (
            select to_currency AS currency from fx_conversion
            union
             select currency AS currency from trade
             union
             select currency AS currency from interest
             union
             select currency AS currency from asset
        ) as all_currencies"
        .to_string();

    let rows = client.query(&statement, &[]).await?;

    let mut used_currencies: Vec<String> = vec![];

    for row in rows {
        used_currencies.push(row.get(0));
    }

    Ok(used_currencies)
}

pub enum EventFilter {
    All,
    TradesOnly,
}

pub async fn events_exist(filter: EventFilter) -> anyhow::Result<bool> {
    let client = db_client().await?;

    let query = match filter {
        EventFilter::All => "
            SELECT 
                EXISTS (SELECT 1 FROM trade LIMIT 1) OR
                EXISTS (SELECT 1 FROM dividend LIMIT 1) OR
                EXISTS (SELECT 1 FROM interest LIMIT 1) OR
                EXISTS (SELECT 1 FROM fx_conversion LIMIT 1) OR
                EXISTS (SELECT 1 FROM asset_trade LIMIT 1) OR
                EXISTS (SELECT 1 FROM asset_transaction LIMIT 1) OR
                EXISTS (SELECT 1 FROM asset_balance_snapshot LIMIT 1)
                AS any_event_table_has_entries
        "
        .to_string(),

        EventFilter::TradesOnly => "
            SELECT
                EXISTS (SELECT 1 FROM trade LIMIT 1) OR
                EXISTS (SELECT 1 FROM asset_trade LIMIT 1) OR
                EXISTS (SELECT 1 FROM asset_transaction LIMIT 1) OR
                EXISTS (SELECT 1 FROM asset_balance_snapshot LIMIT 1) OR
                EXISTS (SELECT 1 FROM interest WHERE asset_id IS NOT NULL LIMIT 1)
                AS table_has_entries
        "
        .to_string(),
    };

    let row = client.query_one(&query, &[]).await?;

    let table_has_entries: bool = row.get(0);

    Ok(table_has_entries)
}

pub async fn get_used_isins() -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;

    let statement: String = "select distinct(isin) from trade".to_string();

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
    super::trade::get_trades(super::QueryFilter {
        limit: count,
        ..Default::default()
    })
    .await
}

pub async fn get_brokers() -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;
    let rows = client
        .query(
            "SELECT DISTINCT broker FROM (
                SELECT broker FROM trade
                UNION
                SELECT broker FROM dividend
                UNION
                SELECT broker FROM interest
                UNION
                SELECT broker FROM fx_conversion
            ) b WHERE broker IS NOT NULL AND btrim(broker) <> '' ORDER BY broker",
            &[],
        )
        .await?;

    Ok(rows.iter().map(|row| row.get::<usize, String>(0)).collect())
}

pub async fn add_trade_to_db(trade: Trade, id: Option<String>) -> anyhow::Result<()> {
    let client = db_client().await?;

    let hash = if let Some(id) = id {
        hash_string(format!("{}{}", trade.broker, id).as_str())
    } else {
        // if the broker doesn't share the id of the trade, hash generation falls back to a
        // combination of trade properties
        // removed the trade.date because revolut apparently changed their timestamp handling in
        // the later versions of their statements
        hash_string(
            format!(
                "{}{}{}{}",
                trade.isin, trade.units, trade.direction, trade.avg_price_per_unit
            )
            .as_str(),
        )
    };

    client.execute(
        "INSERT INTO trade (hash, date, units, avg_price_per_unit, eur_avg_price_per_unit, security_type, direction, currency, isin, broker, date_added, fees, withholding_tax, withholding_tax_currency) values ($1, $2, $3, $4, $5, $6,$7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT(hash) DO NOTHING",
        &[&hash, &trade.date, &trade.units, &trade.avg_price_per_unit, &trade.eur_avg_price_per_unit, &trade.security_type, &trade.direction, &trade.currency, &trade.isin, &trade.broker, &Utc::now(), &trade.fees, &trade.withholding_tax, &trade.withholding_tax_currency],
        )
    .await?;

    println!("✅ Trade added: {:?}", trade);

    // Atomically retain the latest execution fallback without replacing an automatic
    // quote or manual pin. Even older imports can improve a previously absent fallback.
    update_instrument_price(Instrument {
        id: trade.isin.clone(),
        last_price_update: trade.date,
        price: trade.eur_avg_price_per_unit,
        name: trade.isin.clone(),
    })
    .await?;

    Ok(())
}

pub async fn get_active_years() -> anyhow::Result<Vec<i32>> {
    let client = db_client().await?;

    let mut years: Vec<i32> = vec![];

    let rows = client
        .query(
            "WITH all_dates AS (
                SELECT MIN(date) AS earliest_date FROM (
                SELECT date FROM interest
                UNION ALL
                SELECT date FROM trade
                UNION ALL
                SELECT date FROM fx_conversion
                UNION ALL
                SELECT date FROM dividend
                UNION ALL
                SELECT date FROM asset_trade
                UNION ALL
                SELECT date FROM asset_transaction
                UNION ALL
                SELECT date FROM asset_balance_snapshot
                UNION ALL
                SELECT date FROM asset_valuation
                ) AS all_dates
            )
            SELECT 
            GENERATE_SERIES(EXTRACT(YEAR FROM earliest_date)::INT, EXTRACT(YEAR FROM CURRENT_DATE)::INT) AS years
            FROM all_dates;
            ",
            &[],
        )
        .await?;

    for row in rows {
        let year = row.get::<usize, i32>(0);
        years.push(year);
    }

    Ok(years)
}
