use anyhow::anyhow;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::database::db_client;

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

    let query = if currency_from != "EUR" {
        format!(
            "SELECT rate FROM fx_rates WHERE currency_to = '{}' AND date < $1 ORDER BY date desc LIMIT 1",
            currency_from
        )
    } else {
        format!(
            "SELECT rate FROM fx_rates WHERE currency_to = '{}' AND date < $1 ORDER BY date desc LIMIT 1",
            currency_to
        )
    };

    let stmt = client.prepare(&query).await?;
    let rows = client.query(&stmt, &[&date]).await?;

    if rows.is_empty() {
        return Err(anyhow!(
            "Exchange rate not found for the given currencies and date."
        ));
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
        .query("SELECT date FROM fx_rates ORDER BY date DESC LIMIT 1", &[])
        .await?;
    if rows.is_empty() {
        return Ok(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
    }
    let value: NaiveDate = rows[0].get(0);
    Ok(value)
}
