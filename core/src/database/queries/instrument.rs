use rust_decimal::Decimal;

use crate::database::{db_client, models::instrument::Instrument};

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

pub async fn get_current_instrument_price(isin: &str) -> anyhow::Result<Decimal> {
    let instrument = get_instrument_by_id(isin).await?;
    match instrument {
        Some(_) => Ok(instrument.unwrap().price),
        None => panic!("No price found for ISIN {} in instrument table.", isin),
    }
}
