use anyhow::anyhow;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio_postgres::Client;

use crate::{database::db_client, services::market_data::fx_rates::fetch_historic_ecb_rates};

pub async fn get_exchange_rate(
    mut currency_from: &str,
    mut currency_to: &str,
    date: &NaiveDate,
) -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    if currency_from != "EUR" && currency_to != "EUR" {
        return Err(anyhow!("At least one leg needs to be EUR denominated"));
    }

    let mut gbx_fx_rate_adjustment_needed: bool = false;

    if currency_to == "GBX" {
        currency_to = "GBP";
        gbx_fx_rate_adjustment_needed = true;
    }

    if currency_from == "GBX" {
        currency_from = "GBP";
        gbx_fx_rate_adjustment_needed = true;
    }

    async fn is_rate_present(client: &Client, currency: &str) -> anyhow::Result<bool> {
        let query = "SELECT EXISTS (SELECT 1 FROM fx_rate WHERE currency_to = $1)";
        let stmt = client.prepare(query).await?;
        let rows = client.query(&stmt, &[&currency]).await?;
        Ok(rows.first().is_some_and(|row| row.get(0)))
    }

    let mut rates_fetched = false;

    if !is_rate_present(&client, currency_from).await? {
        fetch_historic_ecb_rates(Some(currency_from)).await?;
        rates_fetched = true;
    }
    if !is_rate_present(&client, currency_to).await? {
        fetch_historic_ecb_rates(Some(currency_to)).await?;
        rates_fetched = true;
    }

    let client = if rates_fetched {
        db_client().await?
    } else {
        client
    };

    let query = if currency_from != "EUR" {
        format!(
            "SELECT rate FROM fx_rate WHERE currency_to = '{}' AND date < $1 ORDER BY date desc LIMIT 1",
            currency_from
        )
    } else {
        format!(
            "SELECT rate FROM fx_rate WHERE currency_to = '{}' AND date < $1 ORDER BY date desc LIMIT 1",
            currency_to
        )
    };

    let stmt = client.prepare(&query).await?;
    let rows = client.query(&stmt, &[&date]).await?;

    if rows.is_empty() {
        return Err(anyhow!(format!(
            "Exchange rate not found for the given currencies ({}{}) and date ({:?}).",
            currency_from, currency_to, date
        )));
    }

    let mut rate: Decimal = rows[0].get(0);

    if gbx_fx_rate_adjustment_needed {
        rate *= dec!(100);
    }

    if currency_from != "EUR" {
        Ok(dec!(1.0) / rate)
    } else {
        Ok(rate)
    }
}

pub async fn get_most_recent_rate() -> anyhow::Result<NaiveDate> {
    let client = db_client().await?;

    let rows = client
        .query("SELECT date FROM fx_rate ORDER BY date DESC LIMIT 1", &[])
        .await?;
    if rows.is_empty() {
        return Ok(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
    }
    let value: NaiveDate = rows[0].get(0);
    Ok(value)
}
