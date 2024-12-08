use crate::{
    database::{db_client, models::fx_conversion::FxConversion},
    services::shared::hash_string,
};

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
