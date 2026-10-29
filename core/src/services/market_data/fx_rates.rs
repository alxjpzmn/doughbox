use std::time::Duration;

use anyhow::Context;
use chrono::NaiveDate;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;

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

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let fetch_tasks = currencies_to_fetch.iter().map(|used_currency| {
        let client = &client;
        async move {
            let rates = fetch_rates(
                client,
                &format!(
                    "https://data.ecb.europa.eu/data-detail-api/EXR.D.{}.EUR.SP00.A",
                    used_currency
                ),
            )
            .await
            .with_context(|| format!("Failed to import ECB rates for {used_currency}"))?;
            Ok::<_, anyhow::Error>((used_currency, rates))
        }
    });

    let results = futures::future::join_all(fetch_tasks).await;
    for result in results {
        let (used_currency, rates) = result?;
        for rate in rates {
            let hash = hash_string(&format!("{}{}", rate.date, used_currency));
            db.execute(
                "INSERT INTO fx_rate (hash, date, rate, currency_from, currency_to) values ($1, $2, $3, $4, $5) ON CONFLICT(hash) DO NOTHING",
                &[&hash, &rate.date, &rate.rate, &"EUR", used_currency],
            )
            .await?;
        }
    }

    Ok(())
}

#[derive(Debug)]
struct EcbRate {
    date: NaiveDate,
    rate: Decimal,
}

#[derive(Deserialize)]
struct EcbObservation {
    #[serde(rename = "PERIOD")]
    period: String,
    #[serde(rename = "PERIOD_NAME")]
    period_name: Option<String>,
    #[serde(rename = "OBS", with = "rust_decimal::serde::str_option")]
    rate: Option<Decimal>,
}

fn parse_rates(body: &str) -> anyhow::Result<Vec<EcbRate>> {
    let observations: Vec<EcbObservation> =
        serde_json::from_str(body).context("Invalid ECB rate response")?;
    // Non-trading days are present in the series with OBS: null, not a rate.
    let rates = observations
        .into_iter()
        .filter_map(|observation| {
            observation.rate.map(|rate| {
                // PERIOD timestamps can be the previous UTC day. PERIOD_NAME is
                // the actual observation date; never truncate the UTC timestamp.
                let date = match observation.period_name {
                    Some(name) => NaiveDate::parse_from_str(&name, "%d %b %Y")?,
                    None => NaiveDate::parse_from_str(&observation.period, "%Y-%m-%d")?,
                };
                Ok::<_, chrono::ParseError>(EcbRate { date, rate })
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(!rates.is_empty(), "ECB returned no exchange rates");
    anyhow::ensure!(
        rates.iter().all(|rate| rate.rate > Decimal::ZERO),
        "ECB returned a non-positive exchange rate"
    );
    Ok(rates)
}

async fn fetch_rates(client: &Client, url: &str) -> anyhow::Result<Vec<EcbRate>> {
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    parse_rates(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn malformed_or_empty_ecb_responses_are_failures() {
        let valid = r#"[{"PERIOD":"2026-10-23","OBS":"1.08"}]"#;
        let rates = parse_rates(valid).unwrap();
        assert_eq!(rates[0].rate, Decimal::new(108, 2));
        let actual_response =
            r#"[{"PERIOD":"2026-10-25T23:00:00.000Z","PERIOD_NAME":"26 Oct 2026","OBS":"1.1188"}]"#;
        assert_eq!(
            parse_rates(actual_response).unwrap()[0].date,
            NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
        );
        for body in [
            r#"{"error":"not a rate series"}"#,
            "[]",
            r#"[{"PERIOD":"2026-10-23"}]"#,
            r#"[{"PERIOD":"not-a-date","OBS":"1.08"}]"#,
            r#"[{"PERIOD":"2026-10-23","OBS":"invalid"}]"#,
            r#"[{"PERIOD":"2026-10-23","OBS":"0"}]"#,
        ] {
            assert!(parse_rates(body).is_err(), "{body}");
        }
    }

    #[test]
    fn non_trading_days_are_skipped_without_losing_observation_dates() {
        let rates = parse_rates(
            r#"[{"PERIOD":"2026-10-25T23:00:00.000Z","PERIOD_NAME":"26 Oct 2026","OBS":"1.1188"},
                {"PERIOD":"2026-10-24T22:00:00.000Z","PERIOD_NAME":"25 Oct 2026","OBS":null}]"#,
        )
        .unwrap();
        assert_eq!(rates.len(), 1);
        assert_eq!(
            rates[0].date,
            NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
        );
        assert_eq!(rates[0].rate, Decimal::new(11188, 4));
        assert!(parse_rates(r#"[{"PERIOD":"2026-10-25","OBS":null}]"#).is_err());
    }

    #[tokio::test]
    async fn http_failure_is_not_accepted_even_with_valid_rates_in_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/rates", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            socket.read(&mut request).await.unwrap();
            let body = r#"[{"PERIOD":"2026-10-23","OBS":"1.08"}]"#;
            socket.write_all(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let result = fetch_rates(&Client::new(), &url).await;
        assert!(result.is_err());
        server.await.unwrap();
    }
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
