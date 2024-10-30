use std::{println, str::FromStr, time::Duration};

use anyhow::anyhow;
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use serde_json::Value;
use spinners_rs::{Spinner, Spinners};
use tokio::time::sleep;

#[derive(Deserialize, Debug)]

struct OpenFigiResponseItem {
    ticker: String,
    // _name: String,
    #[serde(alias = "shareClassFIGI")]
    _share_class_figi: String,
}

#[derive(Deserialize, Debug)]
struct OpenFIGIResponse {
    data: Vec<OpenFigiResponseItem>,
}

#[derive(Deserialize, Debug)]
struct PolygonSplitResponseItem {
    split_from: Decimal,
    split_to: Decimal,
    execution_date: String,
}

#[derive(Deserialize, Debug)]
struct PolygonSplitResponse {
    results: Vec<PolygonSplitResponseItem>,
}

use crate::util::{
    db_helpers::{get_used_currencies, seed_fx_rates_db, seed_ticker_conversion_db},
    general_helpers::{get_env_variable, rem_first_and_last},
};

use super::{
    db_helpers::{get_instrument_by_id, ListingChange, StockSplit},
    general_helpers::{hash_string, parse_timestamp},
};

#[derive(Deserialize, Debug)]
pub struct FREDResponse {
    pub observations: Vec<FREDResponseItem>,
}
#[derive(Deserialize, Debug)]
pub struct FREDResponseItem {
    pub value: String,
    date: String,
}

pub async fn get_current_equity_price(isin: &str) -> anyhow::Result<Decimal> {
    let instrument = get_instrument_by_id(isin).await?;
    match instrument {
        Some(_) => Ok(instrument.unwrap().price),
        None => panic!("No price found for ISIN {} in instrument table.", isin),
    }
}

pub async fn get_current_security_name(isin: &str) -> anyhow::Result<String> {
    let instrument = get_instrument_by_id(isin).await?;
    match instrument {
        Some(_) => Ok(instrument.unwrap().name),
        None => Ok("Unknown".to_string()),
    }
}

pub fn get_split_adjusted_units(
    isin: &str,
    no_unadjusted_units: Decimal,
    date: DateTime<Utc>,
    split_information: &mut [StockSplit],
) -> Decimal {
    let relevant_splits = split_information
        .iter()
        .find(|item| isin == item.isin && date.timestamp() < item.ex_date.timestamp());
    if relevant_splits.is_none() {
        return no_unadjusted_units;
    }
    let mut no_adjusted_units = dec!(0.0);
    relevant_splits.into_iter().for_each(|relevant_split| {
        no_adjusted_units +=
            no_unadjusted_units * (relevant_split.to_factor / relevant_split.from_factor)
    });
    no_adjusted_units
}

pub fn get_changed_identifier(identifier: &str, listing_changes: Vec<ListingChange>) -> String {
    let relevant_changes = listing_changes
        .iter()
        .find(|item| item.from_identifier == *identifier);

    match relevant_changes {
        Some(listing_change) => (*listing_change).clone().to_identifier,
        None => identifier.to_string(),
    }
}

pub fn get_split_adjusted_price_per_unit(
    isin: &str,
    unadjusted_price_per_unit: Decimal,
    date: DateTime<Utc>,
    split_information: &mut [StockSplit],
) -> Decimal {
    let relevant_splits = split_information
        .iter()
        .find(|item| isin == item.isin && date.timestamp() < item.ex_date.timestamp());
    if relevant_splits.is_none() {
        return unadjusted_price_per_unit;
    }
    let mut adjusted_price = dec!(0.0);
    relevant_splits.into_iter().for_each(|relevant_split| {
        adjusted_price +=
            unadjusted_price_per_unit / (relevant_split.to_factor / relevant_split.from_factor)
    });
    adjusted_price
}

pub async fn fetch_historic_ecb_rates() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Fetching historic FX rates from ECB");
    sp.start();
    let db = seed_fx_rates_db().await?;
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
    let db = seed_fx_rates_db().await?;
    let rows = db
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
    let amount = if currency_from == "GBX" {
        amount * dec!(100.0)
    } else {
        amount
    };

    let currency_to = if currency_to == "GBX" {
        "GBP"
    } else {
        currency_to
    };
    let currency_from = if currency_from == "GBX" {
        "GBP"
    } else {
        currency_from
    };

    let fx_rate = get_exchange_rate(currency_from, currency_to, date).await?;

    Ok(amount * fx_rate)
}

pub async fn get_exchange_rate(
    currency_from: &str,
    currency_to: &str,
    date: &NaiveDate,
) -> anyhow::Result<Decimal> {
    let client = seed_fx_rates_db().await?;
    if currency_from != "EUR" && currency_to != "EUR" {
        return Err(anyhow!("At least one leg needs to be EUR denominated"));
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

    let rate: Decimal = rows[0].get(0);

    if currency_from != "EUR" {
        Ok(dec!(1.0) / rate)
    } else {
        Ok(rate)
    }
}

pub async fn fetch_fred_data_set(index: &str) -> anyhow::Result<FREDResponse> {
    let client = Client::new();
    let fred_token = get_env_variable("FRED_TOKEN").unwrap();

    let fred_response = client
        .get(format!("https://api.stlouisfed.org/fred/series/observations?series_id={index}&api_key={fred_token}&file_type=json"))
        .send()
        .await?
        .text()
        .await?;

    let fred_response_data = serde_json::from_str::<FREDResponse>(&fred_response)?;
    Ok(fred_response_data)
}

pub async fn get_fred_value_for_date(
    fred_data_set: &FREDResponse,
    date: NaiveDate,
) -> anyhow::Result<Decimal> {
    let mut fetch_latest_value = false;

    if Utc::now().date_naive() == date {
        fetch_latest_value = true
    }

    Ok(if fetch_latest_value {
        fred_data_set
            .observations
            .iter()
            .last()
            .unwrap()
            .value
            .parse::<Decimal>()?
    } else {
        let mut date_found = 0;
        let mut date_to_look_for = date;
        if fred_data_set
            .observations
            .iter()
            .any(|item| item.date == date.format("%Y-%m-%d").to_string())
            && fred_data_set
                .observations
                .iter()
                .find(|item| item.date == date.format("%Y-%m-%d").to_string())
                .unwrap()
                .value
                != "."
        {
        } else {
            while date_found == 0 {
                date_to_look_for -= ChronoDuration::days(1);
                if !fred_data_set
                    .observations
                    .iter()
                    .any(|item| item.date == date_to_look_for.format("%Y-%m-%d").to_string())
                {
                    date_found = 0;
                } else {
                    date_found = 1;
                }
            }
        }

        fred_data_set
            .observations
            .iter()
            .find(|item| item.date == date_to_look_for.format("%Y-%m-%d").to_string())
            .unwrap()
            .value
            .parse::<Decimal>()?
    })
}

pub async fn get_isin_from_symbol(symbol: &str) -> anyhow::Result<String> {
    println!("Getting ISIN for symbol {}...", &symbol);

    let client = seed_ticker_conversion_db().await?;

    let statement = format!(
        "SELECT isin from ticker_conversions where ticker = '{}'",
        symbol
    );

    let result = client.query_one(&statement, &[]).await?;

    Ok(result
        .try_get::<usize, String>(0)
        .unwrap_or("Unidentified".to_string()))
}

pub async fn get_symbol_from_isin(isin: &str, exch_code: Option<&str>) -> anyhow::Result<String> {
    println!("Getting symbol for ISIN {}...", &isin);

    let client = Client::new();

    let open_figi_mapping_response = client
        .post("https://api.openfigi.com/v3/mapping/")
        .json(&serde_json::json!([{
                    "idType":"ID_ISIN",
                    "idValue": isin,
                    "exchCode": exch_code.unwrap_or("US"),
                    "includeUnlistedEquities": true

        }]))
        .send()
        .await?
        .text()
        .await?;

    if open_figi_mapping_response == r#"[{"warning":"No identifier found."}]"# {
        return Ok("NONE_FOUND".to_string());
    }
    if open_figi_mapping_response == r#"[{"error":"Invalid idValue format"}]"# {
        return Ok("NONE_FOUND".to_string());
    }

    let open_figi_mapping_response_data =
        serde_json::from_str::<OpenFIGIResponse>(rem_first_and_last(&open_figi_mapping_response))
            .unwrap();

    // OpenFIGI API is rate limited to 5 requests / minute for unregistered users
    sleep(Duration::from_millis(12000)).await;

    Ok(open_figi_mapping_response_data.data[0].ticker.to_string())
}

pub async fn get_stock_split_information(
    symbol: &str,
    isin: &str,
) -> anyhow::Result<Vec<StockSplit>> {
    // Polygon API is rate limited to 5 requests / minute for free users
    sleep(Duration::from_millis(12000)).await;
    let client = Client::new();

    let polygon_key = get_env_variable("POLYGON_TOKEN").unwrap();

    let polygon_response_body = client
        .get(format!(
            "https://api.polygon.io/v3/reference/splits?ticker={symbol}&apiKey={polygon_key}"
        ))
        .send()
        .await?
        .text()
        .await?;

    let polygon_response_data =
        serde_json::from_str::<PolygonSplitResponse>(&polygon_response_body)?;

    let mut splits_found = vec![];

    for polygon_response_item in polygon_response_data.results {
        let stock_split_information = StockSplit {
            id: hash_string(format!("{}{}", isin, polygon_response_item.execution_date).as_str()),
            ex_date: parse_timestamp(
                format!("{} 16:00:00", &polygon_response_item.execution_date).as_str(),
            )?,
            from_factor: polygon_response_item.split_from,
            to_factor: polygon_response_item.split_to,
            isin: isin.to_string(),
        };
        splits_found.push(stock_split_information)
    }

    Ok(splits_found)
}
