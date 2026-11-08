use std::{collections::BTreeSet, time::Duration};

use anyhow::Context;
use chrono::NaiveDate;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::{
    database::{
        db_client,
        queries::{composite::get_used_currencies_all_users, fx_rate::get_exchange_rate},
    },
    services::shared::util::hash_string,
};

pub async fn fetch_historic_ecb_rates(currency: Option<&str>) -> anyhow::Result<()> {
    let db = db_client().await?;

    let currencies_to_fetch: Vec<_> = if let Some(currency) = currency {
        vec![currency.to_string()]
            .into_iter()
            .filter(|c| c != "EUR" && c != "GBX")
            .collect()
    } else {
        background_import_currencies(&db, get_used_currencies_all_users().await?).await?
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
        store_rates(&db, used_currency, &rates).await?;
    }

    Ok(())
}

fn import_currencies(currencies: impl IntoIterator<Item = String>) -> Vec<String> {
    currencies.into_iter()
        .map(|currency| if currency == "GBX" { "GBP".to_string() } else { currency })
        .filter(|currency| currency != "EUR" && !currency.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

async fn background_import_currencies(
    db: &tokio_postgres::Client,
    used_currencies: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    let cached = db.query(
        "SELECT instrument->>'currency' AS currency
         FROM trading_connection
         CROSS JOIN LATERAL jsonb_array_elements(
             CASE WHEN jsonb_typeof(instruments) = 'array' THEN instruments ELSE '[]'::jsonb END
         ) AS instrument
         WHERE instrument->>'currency' IS NOT NULL
         UNION SELECT currency FROM mifid_latest_price",
        &[],
    ).await?;
    Ok(import_currencies(used_currencies.into_iter().chain(
        cached.into_iter().map(|row| row.get::<_, String>("currency"))
    )))
}

const RATE_BATCH_SIZE: usize = 4096;

async fn store_rates(
    db: &tokio_postgres::Client,
    currency: &str,
    rates: &[EcbRate],
) -> anyhow::Result<()> {
    for batch in rates.chunks(RATE_BATCH_SIZE) {
        let mut hashes = Vec::with_capacity(batch.len());
        let mut dates = Vec::with_capacity(batch.len());
        let mut values = Vec::with_capacity(batch.len());
        for rate in batch {
            hashes.push(hash_string(&format!("{}{}", rate.date, currency)));
            dates.push(rate.date);
            values.push(rate.rate);
        }
        db.execute(
            "INSERT INTO fx_rate (hash, date, rate, currency_from, currency_to)
             SELECT observations.hash, observations.date, observations.rate, 'EUR', $4
             FROM unnest($1::text[], $2::date[], $3::numeric[]) AS observations(hash, date, rate)
             ON CONFLICT (hash) DO NOTHING",
            &[&hashes, &dates, &values, &currency],
        ).await?;
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
    fn background_import_normalizes_pence_and_deduplicates_sources() {
        assert_eq!(
            import_currencies(["EUR", "GBX", "USD", "GBP", "USD", ""].map(str::to_string)),
            vec!["GBP".to_string(), "USD".to_string()]
        );
    }

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn unheld_currencies_are_warmed_and_bulk_history_is_quote_ready() {
        use crate::database::queries::fx_rate::quote_conversion;
        let url = std::env::var("TEST_POSTGRES_URL").expect("Set an isolated TEST_POSTGRES_URL");
        let (db, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(async move { connection.await.unwrap() });
        db.batch_execute(
            r#"CREATE TEMP TABLE trading_connection (instruments JSONB NOT NULL);
               CREATE TEMP TABLE mifid_latest_price (isin TEXT, currency TEXT);
               CREATE TEMP TABLE fx_rate (
                   hash TEXT PRIMARY KEY, date DATE NOT NULL, rate NUMERIC,
                   currency_from TEXT NOT NULL, currency_to TEXT NOT NULL
               );
               INSERT INTO trading_connection VALUES
                   ('[{"currency":"GBX"},{"currency":"USD"},{"currency":"EUR"}]');
               INSERT INTO mifid_latest_price VALUES
                   ('unheld', 'JPY'),
                   ('pence', 'GBX'),
                   ('native', 'EUR');"#,
        ).await.unwrap();
        assert_eq!(
            background_import_currencies(&db, vec!["CHF".to_string(), "USD".to_string()]).await.unwrap(),
            ["CHF", "GBP", "JPY", "USD"].map(str::to_string).to_vec()
        );
        let start = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
        let rates: Vec<EcbRate> = (0..RATE_BATCH_SIZE + 3).map(|day| EcbRate {
            date: start + chrono::Duration::days(day as i64),
            rate: Decimal::new(125, 2),
        }).collect();
        store_rates(&db, "USD", &rates).await.unwrap();
        let last_date = rates.last().unwrap().date;
        store_rates(&db, "USD", &[EcbRate { date: last_date, rate: Decimal::new(999, 0) }]).await.unwrap();
        let count: i64 = db.query_one("SELECT count(*) FROM fx_rate", &[]).await.unwrap().get(0);
        assert_eq!(count, rates.len() as i64);
        let quote = quote_conversion(&db, "USD", "EUR", &(last_date + chrono::Duration::days(2))).await.unwrap();
        assert_eq!(quote.rate, Decimal::new(8, 1));
        assert_eq!(quote.observations[0].as_of, last_date);
        assert_eq!(quote.observations[0].units_per_eur, Decimal::new(125, 2));
    }

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
            let mut received = 0;
            while !request[..received].ends_with(b"\r\n\r\n") {
                let count = socket.read(&mut request[received..]).await.unwrap();
                assert!(count > 0, "incomplete HTTP request headers");
                received += count;
            }
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
