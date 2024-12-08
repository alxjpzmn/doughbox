use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::{
    database::{db_client, models::dividend::Dividend},
    services::shared::util::hash_string,
};

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
