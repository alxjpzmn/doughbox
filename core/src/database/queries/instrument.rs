use std::collections::HashMap;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio_postgres::Client;

use crate::{
    database::{
        db_client,
        models::instrument::{Instrument, InstrumentDetails},
    },
    services::market_data::prices::types::PriceQuote,
};

pub async fn get_all_instruments(client: &Client) -> anyhow::Result<Vec<InstrumentDetails>> {
    let rows = client
        .query("SELECT * FROM instrument ORDER BY name, id", &[])
        .await?;
    let instruments: Vec<_> = rows.iter().map(Instrument::from_row).collect();
    let isins: Vec<_> = instruments
        .iter()
        .map(|instrument| instrument.id.clone())
        .collect();
    let quotes = super::price::quotes(client, &isins).await?;
    Ok(instruments
        .into_iter()
        .zip(quotes)
        .map(|(instrument, quote)| with_quote(instrument, quote))
        .collect())
}

pub async fn get_instrument_by_id(
    client: &Client,
    id: &str,
) -> anyhow::Result<Option<InstrumentDetails>> {
    let row = client
        .query_opt("SELECT * FROM instrument WHERE id = $1", &[&id])
        .await?;
    match row {
        Some(row) => details_for_row(client, &row).await.map(Some),
        None => Ok(None),
    }
}

pub async fn rename_instrument(
    client: &Client,
    id: &str,
    name: &str,
) -> anyhow::Result<Option<InstrumentDetails>> {
    let row = client
        .query_opt(
            "UPDATE instrument SET name = $2 WHERE id = $1 RETURNING *",
            &[&id, &name],
        )
        .await?;
    match row {
        Some(row) => details_for_row(client, &row).await.map(Some),
        None => Ok(None),
    }
}

async fn details_for_row(
    client: &Client,
    row: &tokio_postgres::Row,
) -> anyhow::Result<InstrumentDetails> {
    let instrument = Instrument::from_row(row);
    let quotes = super::price::quotes(client, std::slice::from_ref(&instrument.id)).await?;
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

pub async fn update_instrument_price(instrument: Instrument) -> anyhow::Result<()> {
    let client = db_client().await?;
    record_execution_price(&client, &instrument).await?;
    println!("🔄 Instrument added or updated: {:?}", instrument);
    Ok(())
}

pub(crate) async fn record_execution_price(
    client: &tokio_postgres::Client,
    instrument: &Instrument,
) -> anyhow::Result<()> {
    let query = "
        INSERT INTO instrument (id, last_price_update, price, name, price_source, fallback_price, fallback_at, fallback_source)
        VALUES ($1, $2, $3, $4, 'execution_fallback', $3, $2, 'execution_fallback')
        ON CONFLICT (id) DO UPDATE
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
                &instrument.id,
                &instrument.last_price_update,
                &instrument.price,
                &instrument.name,
            ],
        )
        .await?;

    Ok(())
}

pub async fn batch_get_instrument_prices(isins: &[String]) -> anyhow::Result<Vec<Decimal>> {
    let client = db_client().await?;
    Ok(super::price::quotes(&client, isins)
        .await?
        .into_iter()
        .map(|quote| quote.price.unwrap_or(dec!(0)))
        .collect())
}

pub async fn batch_get_instrument_names(isins: &[String]) -> anyhow::Result<Vec<String>> {
    let client = db_client().await?;
    let query = r#"SELECT id, name FROM instrument WHERE id = ANY($1)"#;
    let stmt = client.prepare(query).await?;

    // Convert Vec<String> to Vec<&str> because tokio_postgres expects slices of &str for string arrays.
    let isins_refs: Vec<&str> = isins.iter().map(String::as_str).collect();

    // Query database
    let rows = client.query(&stmt, &[&isins_refs]).await?;

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
