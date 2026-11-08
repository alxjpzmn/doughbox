use std::collections::HashMap;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio_postgres::Client;

use crate::{
    database::{
        db_client,
        models::auth::UserId,
        models::instrument::{Instrument, InstrumentDetails},
    },
    services::market_data::prices::types::PriceQuote,
};

pub async fn get_all_instruments(user_id: &UserId, client: &Client) -> anyhow::Result<Vec<InstrumentDetails>> {
    let rows = client
        .query("SELECT * FROM instrument WHERE user_id = $1 ORDER BY name, id", &[&user_id.as_str()])
        .await?;
    let instruments: Vec<_> = rows.iter().map(Instrument::from_row).collect();
    let isins: Vec<_> = instruments
        .iter()
        .map(|instrument| instrument.id.clone())
        .collect();
    let quotes = super::price::quotes(user_id, client, &isins).await?;
    Ok(instruments
        .into_iter()
        .zip(quotes)
        .map(|(instrument, quote)| with_quote(instrument, quote))
        .collect())
}

pub async fn get_instrument_by_id(
    user_id: &UserId,
    client: &Client,
    id: &str,
) -> anyhow::Result<Option<InstrumentDetails>> {
    let row = client
        .query_opt("SELECT * FROM instrument WHERE user_id = $1 AND id = $2", &[&user_id.as_str(), &id])
        .await?;
    match row {
        Some(row) => details_for_row(user_id, client, &row).await.map(Some),
        None => Ok(None),
    }
}

pub async fn rename_instrument(
    user_id: &UserId,
    client: &Client,
    id: &str,
    name: &str,
) -> anyhow::Result<Option<InstrumentDetails>> {
    let row = client
        .query_opt(
            "UPDATE instrument SET name = $3 WHERE user_id = $1 AND id = $2 RETURNING *",
            &[&user_id.as_str(), &id, &name],
        )
        .await?;
    match row {
        Some(row) => details_for_row(user_id, client, &row).await.map(Some),
        None => Ok(None),
    }
}

async fn details_for_row(
    user_id: &UserId,
    client: &Client,
    row: &tokio_postgres::Row,
) -> anyhow::Result<InstrumentDetails> {
    let instrument = Instrument::from_row(row);
    let quotes = super::price::quotes(user_id, client, std::slice::from_ref(&instrument.id)).await?;
    let quote = quotes
        .into_iter()
        .next()
        .expect("One quote per requested ISIN");
    Ok(with_quote(instrument, quote))
}

fn with_quote(instrument: Instrument, quote: PriceQuote) -> InstrumentDetails {
    InstrumentDetails {
        id: instrument.id,
        last_price_update: instrument.last_price_update,
        price: instrument.price,
        name: instrument.name,
        quote,
    }
}

pub async fn update_instrument_price(user_id: &UserId, instrument: Instrument) -> anyhow::Result<()> {
    let client = db_client().await?;
    record_execution_price(user_id, &client, &instrument).await?;
    println!("🔄 Instrument added or updated: {:?}", instrument);
    Ok(())
}

pub(crate) async fn record_execution_price(
    user_id: &UserId,
    client: &tokio_postgres::Client,
    instrument: &Instrument,
) -> anyhow::Result<()> {
    let query = "
        INSERT INTO instrument (user_id, id, last_price_update, price, name, price_source, fallback_price, fallback_at, fallback_source)
        VALUES ($1, $2, $3, $4, $5, 'execution_fallback', $4, $3, 'execution_fallback')
        ON CONFLICT (user_id, id) DO UPDATE
        SET fallback_price = CASE WHEN instrument.fallback_at IS NULL OR instrument.fallback_at < EXCLUDED.fallback_at
                                  THEN EXCLUDED.fallback_price ELSE instrument.fallback_price END,
            fallback_at = GREATEST(instrument.fallback_at, EXCLUDED.fallback_at),
            fallback_source = CASE WHEN instrument.fallback_at IS NULL OR instrument.fallback_at < EXCLUDED.fallback_at
                                   THEN 'execution_fallback' ELSE instrument.fallback_source END,
            price = CASE WHEN NOT instrument.price_pinned AND instrument.price_report_id IS NULL AND instrument.price_source <> 'manual'
                              AND instrument.last_price_update < EXCLUDED.last_price_update THEN EXCLUDED.price ELSE instrument.price END,
            last_price_update = CASE WHEN NOT instrument.price_pinned AND instrument.price_report_id IS NULL AND instrument.price_source <> 'manual'
                                          AND instrument.last_price_update < EXCLUDED.last_price_update
                                     THEN EXCLUDED.last_price_update ELSE instrument.last_price_update END,
            price_source = CASE WHEN NOT instrument.price_pinned AND instrument.price_report_id IS NULL AND instrument.price_source <> 'manual'
                                     AND instrument.last_price_update < EXCLUDED.last_price_update
                                THEN 'execution_fallback' ELSE instrument.price_source END;
    ";

    client
        .execute(
            query,
            &[
                &user_id.as_str(),
                &instrument.id,
                &instrument.last_price_update,
                &instrument.price,
                &instrument.name,
            ],
        )
        .await?;

    Ok(())
}

pub async fn batch_get_instrument_prices(user_id: &UserId, isins: &[String]) -> anyhow::Result<Vec<Decimal>> {
    let client = db_client().await?;
    Ok(super::price::quotes(user_id, &client, isins)
        .await?
        .into_iter()
        .map(|quote| quote.price.unwrap_or(dec!(0)))
        .collect())
}

pub async fn batch_get_instrument_names(user_id: &UserId, isins: &[String]) -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;
    let query = r#"SELECT id, name FROM instrument WHERE user_id = $1 AND id = ANY($2)"#;
    let stmt = client.prepare(query).await?;

    // Convert Vec<String> to Vec<&str> because tokio_postgres expects slices of &str for string arrays.
    let isins_refs: Vec<&str> = isins.iter().map(String::as_str).collect();

    // Query database
    let rows = client.query(&stmt, &[&user_id.as_str(), &isins_refs]).await?;

    // Build a HashMap for quick lookups
    let mut name_map = HashMap::new();
    for row in rows {
        let isin: String = row.get(0);
        let name: String = row.get(1);
        name_map.insert(isin, name);
    }

    // Map results back to the input order, using "Unknown" for missing entries
    Ok(isins
        .iter()
        .map(|isin| {
            name_map
                .get(isin)
                .cloned()
                .unwrap_or_else(|| "Unknown".to_string())
        })
        .collect())
}
