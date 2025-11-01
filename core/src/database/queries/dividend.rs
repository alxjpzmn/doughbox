use crate::{
    database::{db_client, models::dividend::Dividend},
    services::shared::util::hash_string,
};

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
            "INSERT INTO dividend (id, isin, date, amount, broker, currency, amount_eur, withholding_tax, withholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &dividend.isin, &dividend.date, &dividend.amount, &dividend.broker, &dividend.currency, &dividend.amount_eur, &dividend.withholding_tax, &dividend.withholding_tax_currency],
        )
    .await?;
    println!("💵 Dividend added: {:?}", dividend);

    Ok(())
}
