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

pub async fn fetch_historic_ecb_rates(currency: Option<&str>) -> anyhow::Result<()> {
    let db = db_client().await?;
    let used_currencies = get_used_currencies().await?;

    let currencies_to_fetch: Vec<_> = if let Some(currency) = currency {
        vec![currency.to_string()]
            .into_iter()
            .filter(|c| c != "EUR" && c != "GBX")
            .collect()
    } else {
        used_currencies
            .into_iter()
            .filter(|c| c != "EUR" && c != "GBX")
            .collect()
    };

    let fetch_tasks = currencies_to_fetch.iter().map(|used_currency| async move {
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
        Ok::<_, anyhow::Error>((used_currency.clone(), data))
    });

    let results = futures::future::join_all(fetch_tasks).await;

    for result in results {
        match result {
            Ok((used_currency, data)) => {
                if let Some(exchange_rates) = data.as_array() {
                    for exchange_rate in exchange_rates {
                        if let (Some(date_str), Some(rate_str)) = (
                            exchange_rate.get("PERIOD").and_then(|v| v.as_str()),
                            exchange_rate.get("OBS").and_then(|v| v.as_str()),
                        ) {
                            let date = NaiveDate::from_str(date_str)?;
                            let rate = rate_str.parse::<Decimal>().unwrap_or(dec!(0.0));

                            if rate != dec!(0.0) {
                                let hash = hash_string(&format!("{}{}", date, used_currency));
                                db.execute(
                                    "INSERT INTO fx_rate (hash, date, rate, currency_from, currency_to) values ($1, $2, $3, $4, $5) ON CONFLICT(hash) DO NOTHING",
                                    &[&hash, &date, &rate, &"EUR".to_string(), &used_currency],
                                )
                                .await?;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to fetch data for a currency: {}", e);
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
