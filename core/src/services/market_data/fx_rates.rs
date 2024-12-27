use std::str::FromStr;

use chrono::NaiveDate;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::Value;

use crate::{
    database::{
        db_client,
        queries::{composite::get_used_currencies, fx_rate::get_exchange_rate},
    },
    services::shared::util::hash_string,
};

pub async fn fetch_historic_ecb_rates() -> anyhow::Result<()> {
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
                    "INSERT INTO fx_rate (hash, date, rate, currency_from, currency_to) values ($1, $2, $3, $4, $5) ON CONFLICT(hash) DO NOTHING",
                    &[&hash, &date, &rate, &currency_from, &currency_to],
                )
            .await?;
            }
        }
    }
    Ok(())
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
