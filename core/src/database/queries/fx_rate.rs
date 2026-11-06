use anyhow::{anyhow, ensure};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use tokio_postgres::Client;

use crate::{database::db_client, services::market_data::fx_rates::fetch_historic_ecb_rates};

/// A stored ECB observation, in major currency units per EUR.
#[derive(Clone, Debug, Serialize)]
pub struct FxObservation {
    pub currency: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub units_per_eur: Decimal,
    pub as_of: NaiveDate,
}

#[derive(Clone, Debug, Serialize)]
pub struct QuoteConversion {
    #[serde(with = "rust_decimal::serde::str")]
    pub rate: Decimal,
    pub observations: Vec<FxObservation>,
}

fn major_currency(currency: &str) -> &str {
    if currency == "GBX" { "GBP" } else { currency }
}

fn unit_conversion(from: &str, to: &str) -> Option<Decimal> {
    if from == to {
        Some(Decimal::ONE)
    } else if from == "GBP" && to == "GBX" {
        Some(dec!(100))
    } else if from == "GBX" && to == "GBP" {
        Some(dec!(0.01))
    } else {
        None
    }
}

/// Convert using only observations already on file. Never imports or performs HTTP.
pub async fn quote_conversion(
    client: &Client,
    from: &str,
    to: &str,
    date: &NaiveDate,
) -> anyhow::Result<QuoteConversion> {
    if let Some(rate) = unit_conversion(from, to) {
        return Ok(QuoteConversion { rate, observations: Vec::new() });
    }
    let source = major_currency(from);
    let target = major_currency(to);
    let currencies: Vec<&str> = [source, target]
        .into_iter()
        .filter(|currency| *currency != "EUR")
        .collect();
    let rows = client.query(
        "SELECT requested.currency, observation.rate, observation.date
         FROM unnest($1::text[]) WITH ORDINALITY AS requested(currency, position)
         CROSS JOIN LATERAL (
             SELECT rate, date FROM fx_rate
             WHERE currency_from = 'EUR' AND currency_to = requested.currency AND date <= $2
             ORDER BY date DESC, hash DESC LIMIT 1
         ) observation
         ORDER BY requested.position",
        &[&currencies, date],
    ).await?;
    let observations: Vec<FxObservation> = rows.into_iter().map(|row| {
        Ok(FxObservation {
            currency: row.try_get("currency")?,
            units_per_eur: row.try_get("rate")
                .map_err(|_| anyhow!("Stored FX observation is null or outside supported decimal precision"))?,
            as_of: row.try_get("date")?,
        })
    }).collect::<anyhow::Result<_>>()?;
    let stored_rate = |currency: &str| -> anyhow::Result<Decimal> {
        if currency == "EUR" {
            return Ok(Decimal::ONE);
        }
        let observation = observations.iter().find(|rate| rate.currency == currency)
            .ok_or_else(|| anyhow!("EUR/{currency} exchange rate not found on or before {date}"))?;
        ensure!(observation.units_per_eur > Decimal::ZERO,
            "EUR/{currency} exchange rate must be positive");
        Ok(observation.units_per_eur)
    };
    let source_rate = stored_rate(source)?;
    let target_rate = stored_rate(target)?;
    let rate = target_rate.checked_div(source_rate)
        .and_then(|rate| if from == "GBX" { rate.checked_div(dec!(100)) } else { Some(rate) })
        .and_then(|rate| if to == "GBX" { rate.checked_mul(dec!(100)) } else { Some(rate) })
        .filter(|rate| *rate > Decimal::ZERO)
        .ok_or_else(|| anyhow!("{from}/{to} exchange rate cannot be represented"))?;
    Ok(QuoteConversion { rate, observations })
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

    let currency = if currency_from != "EUR" {
        currency_from
    } else {
        currency_to
    };
    let rows = client
        .query(
            "SELECT rate FROM fx_rate WHERE currency_to = $1 AND date <= $2 ORDER BY date DESC LIMIT 1",
            &[&currency, &date],
        )
        .await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_and_pence_conversion_need_no_observations() {
        assert_eq!(unit_conversion("USD", "USD"), Some(Decimal::ONE));
        assert_eq!(unit_conversion("EUR", "EUR"), Some(Decimal::ONE));
        assert_eq!(unit_conversion("GBP", "GBX"), Some(dec!(100)));
        assert_eq!(unit_conversion("GBX", "GBP"), Some(dec!(0.01)));
        assert_eq!(unit_conversion("GBX", "GBX"), Some(Decimal::ONE));
        assert_eq!(unit_conversion("USD", "JPY"), None);
    }

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn stored_quote_conversions_preserve_direction_dates_and_failures() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("Set an isolated TEST_POSTGRES_URL");
        let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await.unwrap();
        let connection_task = tokio::spawn(async move { connection.await.unwrap() });
        client.batch_execute(
            "CREATE TEMP TABLE fx_rate (
                hash TEXT PRIMARY KEY, date DATE NOT NULL, rate NUMERIC,
                currency_from TEXT NOT NULL, currency_to TEXT NOT NULL
             );
             INSERT INTO fx_rate VALUES
                ('gbp', '2026-10-23', 0.8, 'EUR', 'GBP'),
                ('usd', '2026-10-22', 1.25, 'EUR', 'USD'),
                ('jpy', '2026-10-23', 150, 'EUR', 'JPY'),
                ('future', '2026-10-26', 200, 'EUR', 'JPY'),
                ('wrong-base', '2026-10-24', 999, 'USD', 'JPY'),
                ('zero', '2026-10-23', 0, 'EUR', 'ZERO'),
                ('negative', '2026-10-23', -1, 'EUR', 'NEG'),
                ('huge', '2026-10-23', 1e40, 'EUR', 'HUGE'),
                ('max', '2026-10-23', 79228162514264337593543950335, 'EUR', 'MAX'),
                ('tiny', '2026-10-23', 0.0000000000000000000000000001, 'EUR', 'TINY'),
                ('null', '2026-10-23', NULL, 'EUR', 'NULL')",
        ).await.unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let converted = quote_conversion(&client, "USD", "JPY", &date).await.unwrap();
        assert_eq!(converted.rate, dec!(120));
        assert_eq!(converted.observations.len(), 2);
        assert_eq!(converted.observations[0].currency, "USD");
        assert_eq!(converted.observations[0].as_of, NaiveDate::from_ymd_opt(2026, 10, 22).unwrap());
        assert_eq!(converted.observations[1].currency, "JPY");
        assert_eq!(converted.observations[1].as_of, NaiveDate::from_ymd_opt(2026, 10, 23).unwrap());
        assert_eq!(quote_conversion(&client, "JPY", "USD", &date).await.unwrap().rate,
            dec!(1.25).checked_div(dec!(150)).unwrap());
        for (from, to, expected) in [
            ("EUR", "USD", dec!(1.25)), ("USD", "EUR", dec!(0.8)),
            ("EUR", "GBX", dec!(80)), ("GBX", "EUR", dec!(0.0125)),
            ("USD", "GBX", dec!(64)), ("GBX", "USD", dec!(0.015625)),
        ] {
            assert_eq!(quote_conversion(&client, from, to, &date).await.unwrap().rate, expected);
        }
        for currency in ["MISSING", "ZERO", "NEG", "HUGE", "NULL", "GBP' OR TRUE --"] {
            for (from, to) in [("EUR", currency), (currency, "EUR")] {
                let error = quote_conversion(&client, from, to, &date).await.unwrap_err();
                assert!(!error.is::<tokio_postgres::Error>(),
                    "Invalid stored observations must stay per-quote conversion failures: {currency}");
            }
        }
        assert!(quote_conversion(&client, "TINY", "MAX", &date).await.is_err());
        assert!(quote_conversion(&client, "MAX", "TINY", &date).await.is_err());
        let before = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        assert!(quote_conversion(&client, "USD", "JPY", &before).await.is_err());

        // A closed database connection proves native/unit-only reads do not query it.
        connection_task.abort();
        let _ = connection_task.await;
        for (from, to, rate) in [
            ("USD", "USD", Decimal::ONE), ("GBP", "GBX", dec!(100)),
            ("GBX", "GBP", dec!(0.01)), ("GBX", "GBX", Decimal::ONE),
        ] {
            let conversion = quote_conversion(&client, from, to, &date).await.unwrap();
            assert_eq!(conversion.rate, rate);
            assert!(conversion.observations.is_empty());
        }
    }
}
