use std::collections::{HashMap, HashSet};

use anyhow::{ensure, Context};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::database::{
    db_client,
    queries::{
        fx_rate,
        price::{self, LatestPrice},
    },
};

use super::provider::Instrument;

#[derive(Clone, Serialize)]
struct CurrencyConversion {
    from: String,
    to: String,
    #[serde(with = "rust_decimal::serde::str")]
    rate: Decimal,
    observations: Vec<fx_rate::FxObservation>,
    source: &'static str,
}

#[derive(Serialize)]
struct TradeReference {
    #[serde(with = "rust_decimal::serde::str")]
    price: Decimal,
    #[serde(with = "rust_decimal::serde::str_option")]
    quantity: Option<Decimal>,
    currency: String,
    feed: String,
    source: &'static str,
    venue: String,
    as_of: DateTime<Utc>,
    received_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct DelayedQuote {
    ticker: String,
    isin: String,
    name: String,
    currency: String,
    #[serde(with = "rust_decimal::serde::str_option")]
    price: Option<Decimal>,
    reference: Option<TradeReference>,
    age_seconds: Option<i64>,
    stale: bool,
    indicative: bool,
    conversion: Option<CurrencyConversion>,
    unavailable_reason: Option<String>,
}

#[derive(Serialize)]
pub(super) struct QuoteBatch {
    delayed: bool,
    reference_type: &'static str,
    max_age_seconds: i32,
    quotes: Vec<DelayedQuote>,
    notice: &'static str,
}

fn currency_rank(source: &str, requested: &str) -> u8 {
    if source == requested || matches!((source, requested), ("GBP", "GBX") | ("GBX", "GBP")) {
        0
    } else if source == "EUR" {
        1
    } else {
        2
    }
}

fn select<'a>(
    references: &'a [LatestPrice],
    instrument: &Instrument,
    now: DateTime<Utc>,
    max_age: Duration,
) -> Option<&'a LatestPrice> {
    references
        .iter()
        .filter(|reference| reference.isin == instrument.isin)
        .min_by(|a, b| {
            let rank = |reference: &LatestPrice| {
                (
                    reference.quantity == Some(Decimal::ZERO),
                    now - reference.as_of > max_age,
                    currency_rank(&reference.currency, &instrument.currency),
                )
            };
            rank(a)
                .cmp(&rank(b))
                .then_with(|| b.as_of.cmp(&a.as_of))
                .then_with(|| a.feed.cmp(&b.feed))
                .then_with(|| a.currency.cmp(&b.currency))
                .then_with(|| a.venue.cmp(&b.venue))
        })
}

fn native_quote(
    instrument: &Instrument,
    references: &[LatestPrice],
    now: DateTime<Utc>,
    max_age: Duration,
) -> DelayedQuote {
    let mut quote = DelayedQuote {
        ticker: instrument.ticker.clone(),
        isin: instrument.isin.clone(),
        name: instrument.name.clone(),
        currency: instrument.currency.clone(),
        price: None,
        reference: None,
        age_seconds: None,
        stale: true,
        indicative: false,
        conversion: None,
        unavailable_reason: None,
    };
    let Some(selected) = select(references, instrument, now, max_age) else {
        quote.unavailable_reason = Some("No eligible bulk-ingested last-trade reference from the past 168 hours is available for this ISIN. Coverage depends on completed price-feed imports.".into());
        return quote;
    };
    quote.age_seconds = Some((now - selected.as_of).num_seconds().max(0));
    quote.stale = now - selected.as_of > max_age;
    quote.indicative = selected.quantity == Some(Decimal::ZERO);
    quote.price = Some(selected.price);
    quote.reference = Some(TradeReference {
        price: selected.price,
        quantity: selected.quantity,
        currency: selected.currency.clone(),
        feed: selected.feed.clone(),
        source: "Bulk post-trade files",
        venue: selected.venue.clone(),
        as_of: selected.as_of,
        received_at: selected.received_at,
    });
    quote
}

fn convert_price(quote: &mut DelayedQuote, conversion: CurrencyConversion) {
    quote.price = quote.price.and_then(|value| {
        value.checked_mul(conversion.rate).filter(|value| *value > Decimal::ZERO)
    });
    if quote.price.is_none() {
        quote.unavailable_reason =
            Some("Converted last-trade price exceeds supported decimal precision.".into());
    }
    quote.conversion = Some(conversion);
}

fn resolve_instruments<'a>(
    catalog: &'a [Instrument],
    tickers: &[String],
) -> anyhow::Result<Vec<&'a Instrument>> {
    let requested: HashSet<_> = tickers.iter().map(String::as_str).collect();
    let matches: HashMap<_, _> = catalog
        .iter()
        .filter(|instrument| requested.contains(instrument.ticker.as_str()))
        .map(|instrument| (instrument.ticker.as_str(), instrument))
        .collect();
    let instruments = tickers
        .iter()
        .map(|ticker| {
            matches.get(ticker.as_str()).copied().with_context(|| {
                format!("Unknown Trading 212 ticker: {ticker}. Use search_instruments first.")
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    for instrument in &instruments {
        ensure!(
            instrument.isin.len() == 12
                && instrument.isin.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "Trading 212 has an invalid ISIN for a requested ticker."
        );
    }
    Ok(instruments)
}

pub(super) async fn get(
    hash: &str,
    tickers: &[String],
    max_age_seconds: i32,
) -> anyhow::Result<QuoteBatch> {
    ensure!(
        (1..=50).contains(&tickers.len()),
        "Request between 1 and 50 Trading 212 tickers."
    );
    ensure!(max_age_seconds > 0, "max_age_seconds must be positive.");
    ensure!(
        tickers.iter().all(|ticker| (1..=100).contains(&ticker.len())),
        "Use exact Trading 212 tickers from search_instruments."
    );
    let catalog = super::catalog::load(hash).await?;
    let instruments = resolve_instruments(&catalog.instruments, tickers)?;
    let client = db_client().await?;
    let isins: Vec<_> = instruments.iter().map(|instrument| instrument.isin.clone()).collect();
    let references = price::latest_prices(&client, &isins).await?;
    let now = Utc::now();
    let max_age = Duration::seconds(i64::from(max_age_seconds));
    let mut conversions: HashMap<(String, String, NaiveDate), Result<CurrencyConversion, String>> =
        HashMap::new();
    let mut quotes = Vec::with_capacity(instruments.len());
    for instrument in instruments {
        let mut quote = native_quote(instrument, &references, now, max_age);
        if let Some(reference) = quote.reference.as_ref().filter(|reference| {
            reference.currency != quote.currency && quote.price.is_some()
        }) {
            let key = (
                reference.currency.clone(),
                quote.currency.clone(),
                reference.as_of.date_naive(),
            );
            let conversion = if let Some(cached) = conversions.get(&key) {
                cached.clone()
            } else {
                let result = match fx_rate::quote_conversion(&client, &key.0, &key.1, &key.2).await {
                    Ok(conversion) => Ok(CurrencyConversion {
                        from: key.0.clone(),
                        to: key.1.clone(),
                        rate: conversion.rate,
                        source: if conversion.observations.is_empty() {
                            "Currency unit conversion"
                        } else {
                            "ECB daily reference rates"
                        },
                        observations: conversion.observations,
                    }),
                    Err(error) if error.is::<tokio_postgres::Error>() => return Err(error),
                    Err(error) => Err(error.to_string()),
                };
                conversions.insert(key, result.clone());
                result
            };
            match conversion {
                Ok(conversion) => convert_price(&mut quote, conversion),
                Err(error) => {
                    quote.price = None;
                    quote.unavailable_reason = Some(format!(
                        "Stored {} last-trade reference is available, but conversion to {} is unavailable: {error}. Refresh ECB reference data under Data.",
                        reference.currency, quote.currency,
                    ));
                }
            }
        }
        quotes.push(quote);
    }
    Ok(QuoteBatch {
        delayed: true,
        reference_type: "last_trade",
        max_age_seconds,
        quotes,
        notice: "DELAYED bulk-ingested last-trade references, not live, executable, bid/ask or NBBO quotes. A zero reported quantity is indicative. Inspect native feed, venue, event time, age_seconds, stale, indicative and unavailable_reason; receipt time does not make an old trade fresh. References expire 168 hours after source events regardless of max_age_seconds. Prices and dated ECB observations or exact GBP/GBX units are read from storage: no exchange quote HTTP, price jobs or FX imports. Older broker catalogs may require metadata backfill. Coverage depends on completed bulk-feed imports. Choose explicit ticker-currency limit prices for human review; quotes never choose, submit or reprice orders.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn now() -> DateTime<Utc> {
        "2026-11-01T12:00:00Z".parse().unwrap()
    }

    fn instrument(currency: &str) -> Instrument {
        Instrument {
            ticker: "FIXTURE_EQ".into(),
            name: "Fixture".into(),
            currency: currency.into(),
            isin: "DE0007164600".into(),
        }
    }

    fn reference(currency: &str, time: DateTime<Utc>) -> LatestPrice {
        LatestPrice {
            isin: "DE0007164600".into(),
            currency: currency.into(),
            price: dec!(125.700000000000000001),
            feed: "DGAT-posttrade".into(),
            venue: "XGAT".into(),
            as_of: time,
            received_at: now(),
            quantity: Some(dec!(12)),
        }
    }

    fn quote(references: &[LatestPrice]) -> DelayedQuote {
        native_quote(&instrument("EUR"), references, now(), Duration::seconds(1800))
    }

    #[test]
    fn last_trade_contract_preserves_decimal_precision_without_book_aliases() {
        let result = quote(&[reference("EUR", now() - Duration::minutes(17))]);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["price"], "125.700000000000000001");
        for removed in ["bid", "ask", "midpoint", "last_known", "refresh_error"] {
            assert!(json.get(removed).is_none());
        }
    }

    #[test]
    fn selection_prefers_non_indicative_then_fresh_then_native_units() {
        let mut indication = reference("GBP", now() - Duration::minutes(16));
        indication.quantity = Some(Decimal::ZERO);
        let stale = reference("GBX", now() - Duration::hours(1));
        let fresh = reference("EUR", now() - Duration::minutes(18));
        let mut references = vec![indication, stale, fresh];
        let requested = instrument("GBP");
        let selected = select(&references, &requested, now(), Duration::minutes(30)).unwrap();
        assert_eq!(selected.currency, "EUR");
        references[1].as_of = now() - Duration::minutes(20);
        let selected = select(&references, &requested, now(), Duration::minutes(30)).unwrap();
        assert_eq!(selected.currency, "GBX");
        references[1].quantity = None;
        assert_eq!(select(&references, &requested, now(), Duration::minutes(30)).unwrap().currency, "GBX");
    }

    #[test]
    fn indicative_and_unavailable_references_are_explicit() {
        let mut indication = reference("EUR", now() - Duration::minutes(17));
        indication.quantity = Some(Decimal::ZERO);
        let result = quote(&[indication]);
        assert!(result.indicative);
        assert!(result.price.is_some() && !result.stale);
        assert_eq!(result.reference.unwrap().quantity, Some(Decimal::ZERO));
        let missing = quote(&[]);
        assert!(missing.price.is_none() && missing.reference.is_none());
        assert!(missing.stale && missing.age_seconds.is_none());
        assert!(missing.unavailable_reason.is_some());
    }

    #[test]
    fn source_event_time_controls_age_and_staleness() {
        let mut native = reference("EUR", now() - Duration::minutes(30));
        let result = quote(std::slice::from_ref(&native));
        assert_eq!(result.age_seconds, Some(1800));
        assert!(!result.stale);
        native.as_of -= Duration::nanoseconds(1);
        assert!(quote(&[native]).stale);
    }

    #[test]
    fn latest_event_and_deterministic_feed_break_selection_ties() {
        let old = reference("EUR", now() - Duration::minutes(20));
        let mut latest = reference("EUR", now() - Duration::minutes(17));
        latest.feed = "EURONEXT-posttrade".into();
        let mut tied = reference("EUR", latest.as_of);
        tied.feed = "CBOE-posttrade".into();
        let result = quote(&[old, latest, tied]);
        assert_eq!(result.reference.unwrap().feed, "CBOE-posttrade");
    }

    #[test]
    fn exact_conversion_preserves_native_reference_and_reports_overflow() {
        let references = [reference("EUR", now() - Duration::minutes(17))];
        let mut result = native_quote(&instrument("USD"), &references, now(), Duration::minutes(30));
        let conversion = CurrencyConversion {
            from: "EUR".into(),
            to: "USD".into(),
            rate: dec!(1.10),
            observations: vec![],
            source: "fixture",
        };
        convert_price(&mut result, conversion.clone());
        assert_eq!(result.price, Some(dec!(138.27000000000000000110)));
        assert_eq!(result.reference.as_ref().unwrap().price, references[0].price);
        let mut overflow = quote(&references);
        convert_price(&mut overflow, CurrencyConversion { rate: Decimal::MAX, ..conversion });
        assert!(overflow.price.is_none() && overflow.unavailable_reason.is_some());
        assert_eq!(overflow.reference.unwrap().price, references[0].price);
    }

    #[test]
    fn exact_ticker_mapping_preserves_currency_order_and_duplicates() {
        let pence = instrument("GBX");
        let mut dollars = instrument("USD");
        dollars.ticker = "DOLLARS_EQ".into();
        let catalog = vec![pence, dollars];
        let requested = ["DOLLARS_EQ", "FIXTURE_EQ", "DOLLARS_EQ"].map(str::to_string);
        let resolved = resolve_instruments(&catalog, &requested).unwrap();
        assert_eq!(resolved.iter().map(|instrument| instrument.currency.as_str()).collect::<Vec<_>>(), vec!["USD", "GBX", "USD"]);
        assert!(resolve_instruments(&catalog, &["fixture_eq".into()]).is_err());
        let mut invalid = instrument("EUR");
        invalid.isin = "invalid".into();
        assert!(resolve_instruments(&[invalid], &["FIXTURE_EQ".into()]).is_err());
    }
}
