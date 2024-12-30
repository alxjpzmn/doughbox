use std::collections::HashMap;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::database::{db_client, models::instrument::Instrument};

pub async fn get_instrument_by_id(id: &str) -> anyhow::Result<Option<Instrument>> {
    let client = db_client().await?;

    let row = client
        .query_opt("SELECT * FROM instrument WHERE id = $1", &[&id])
        .await?;

    match row {
        Some(row) => Ok(Some(Instrument::from_row(&row))),
        None => Ok(None),
    }
}

pub async fn update_instrument_price(instrument: Instrument) -> anyhow::Result<()> {
    let client = db_client().await?;

    let query = "
        INSERT INTO instrument (id, last_price_update, price, name)
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

pub async fn batch_get_instrument_prices(isins: &[String]) -> anyhow::Result<Vec<Decimal>> {
    let client = db_client().await?;
    let query = r#"SELECT id, price FROM instrument WHERE id = ANY($1)"#.to_string();
    let stmt = client.prepare(&query).await?;
    let rows = client.query(&stmt, &[&isins]).await?;

    // Map results back to a vector of prices in the same order as `isins`
    let mut price_map = HashMap::new();
    for row in rows {
        let isin: String = row.get(0);
        let price: Decimal = row.get(1);
        price_map.insert(isin, price);
    }
    Ok(isins
        .iter()
        .map(|isin| *price_map.get(isin).unwrap_or(&dec!(0.0)))
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
