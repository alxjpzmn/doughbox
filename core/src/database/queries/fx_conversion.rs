use crate::{
    database::{db_client, models::fx_conversion::FxConversion, queries::QueryFilter},
    services::shared::util::hash_string,
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
            "INSERT INTO fx_conversion (id, date, broker, from_amount, to_amount, from_currency, to_currency, date_added, fees) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &fx_conversion.date, &fx_conversion.broker, &fx_conversion.from_amount, &fx_conversion.to_amount, &fx_conversion.from_currency, &fx_conversion.to_currency, &fx_conversion.date_added, &fx_conversion.fees],
        )
    .await?;

    Ok(())
}

pub async fn get_fx_conversions(filter: QueryFilter) -> anyhow::Result<Vec<FxConversion>> {
    let client = db_client().await?;

    let mut statement = String::from(
        "SELECT date, broker, from_amount, to_amount, from_currency, to_currency, \
         date_added, fees FROM fx_conversion WHERE 1=1",
    );
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
    super::append_common_filters(&mut statement, &mut params, &filter);
    statement.push_str(" ORDER BY date DESC");

    let rows = client.query(&statement, &params).await?;
    Ok(rows
        .iter()
        .map(|row| FxConversion {
            date: row.get("date"),
            broker: row.get("broker"),
            from_amount: row.get("from_amount"),
            to_amount: row.get("to_amount"),
            from_currency: row.get("from_currency"),
            to_currency: row.get("to_currency"),
            date_added: row.get("date_added"),
            fees: row.get("fees"),
        })
        .collect())
}
