use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::{
    database::{
        db_client,
        models::{instrument::Instrument, trade::Trade},
        queries::instrument::{get_instrument_by_id, update_instrument_price},
    },
    services::{instruments::identifiers::get_changed_identifier, shared::util::hash_string},
};

use super::listing_change::get_listing_changes;

pub async fn get_used_currencies() -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;

    let statement: String = "
        select distinct currency from (
            select to_currency AS currency from fx_conversion
            union
            select currency_denomination AS currency from trade
            union
            select currency AS currency from interest
        ) as all_currencies"
        .to_string();

    let rows = client.query(&statement, &[]).await?;

    let mut used_currencies: Vec<String> = vec![];

    for row in rows {
        used_currencies.push(row.get(0));
    }

    Ok(used_currencies)
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
    let client = db_client().await?;

    let mut statement: String = "select * from trade order by date desc".to_string();

    statement = match count {
        Some(_) => {
            let stmt_w_count = format!("{} limit {}", statement, count.unwrap());
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
        "INSERT INTO trade (hash, date, no_units, avg_price_per_unit, eur_avg_price_per_unit, security_type, direction, currency_denomination, isin, broker, date_added, fees, withholding_tax, witholding_tax_currency) values ($1, $2, $3, $4, $5, $6,$7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT(hash) DO NOTHING",
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
