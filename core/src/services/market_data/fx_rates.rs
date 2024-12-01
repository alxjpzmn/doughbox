use std::str::FromStr;

use anyhow::anyhow;
use chrono::NaiveDate;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::Value;
use spinners_rs::{Spinner, Spinners};

use crate::{
    services::shared::hash_string,
    util::db_helpers::{db_client, get_used_currencies},
};

pub async fn fetch_historic_ecb_rates() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Fetching historic FX rates from ECB");
    sp.start();
    let db = db_client().await?;
    let used_currencies = get_used_currencies().await?;

    for used_currency in used_currencies {
        if used_currency == "EUR" || used_currency == "GBX" {
            continue;
        }
        let client = Client::new();
        let res = client
            .get(format!(
                "https://data.ecb.europa.eu/data-detail-api/EXR.D.{}.EUR.SP00.A",
                used_currency
            ))
            .send()
            .await?;

        let body = res.text().await?;

        let data: Value = serde_json::from_str(&body)?;
        let exchange_rates = data.as_array().unwrap().iter();

        for exchange_rate in exchange_rates {
            let date_str = exchange_rate["PERIOD"].as_str().unwrap();
            let date = NaiveDate::from_str(date_str)?;
            let rate = exchange_rate["OBS"]
                .as_str()
                .unwrap_or("0.0")
                .parse::<Decimal>()?;

            let hash = hash_string(format!("{}{}", date, used_currency).as_str());

            let currency_from = "EUR".to_string();
            let currency_to = used_currency.clone();

            if rate != dec!(0.0) {
                db.execute(
                    "INSERT INTO fx_rates (hash, date, rate, currency_from, currency_to) values ($1, $2, $3, $4, $5) ON CONFLICT(hash) DO NOTHING",
                    &[&hash, &date, &rate, &currency_from, &currency_to],
                )
            .await?;
            }
        }
    }
    sp.stop();
    Ok(())
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

pub async fn convert_amount(
    amount: Decimal,
    date: &NaiveDate,
    currency_from: &str,
    currency_to: &str,
) -> anyhow::Result<Decimal> {
    if currency_from == "EUR" && currency_to == "EUR" {
        return Ok(amount);
    }

    let fx_rate = get_exchange_rate(currency_from, currency_to, date).await?;

    Ok(amount * fx_rate)
}

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
