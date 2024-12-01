use std::{println, time::Duration};

use chrono::{DateTime, Utc};
use itertools::Itertools;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use tokio::time::sleep;

use crate::{
    services::{env::get_env_variable, parsers::parse_timestamp, shared::hash_string},
    util::db_helpers::{add_stock_split_to_db, get_stock_splits, get_used_isins},
};

use super::{fund_data::query_for_oekb_funds_data, ticker_symbols::get_symbol_from_isin};

#[derive(Debug, Clone)]
pub struct StockSplit {
    pub id: String,
    pub ex_date: DateTime<Utc>,
    pub from_factor: Decimal,
    pub to_factor: Decimal,
    pub isin: String,
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

pub async fn update_stock_splits() -> anyhow::Result<()> {
    let existing_splits = get_stock_splits().await?;
    let isins = get_used_isins().await?;
    for isin in isins {
        query_for_oekb_funds_data(&isin).await?;
        let symbol = get_symbol_from_isin(&isin, None).await?;
        let split_events = get_stock_split_information(&symbol, &isin).await?;

        for split_event in split_events {
            let split_event_already_stored = !existing_splits
                .clone()
                .into_iter()
                .filter(|item| {
                    item.ex_date.date_naive() == split_event.ex_date.date_naive()
                        && item.isin == split_event.isin
                })
                .collect_vec()
                .is_empty();
            if split_event_already_stored {
                println!(
                    "Split event for {} on {} already stored, skipping.",
                    split_event.isin,
                    split_event.ex_date.date_naive()
                )
            } else {
                println!(
                    "Storing split event for {} on {}.",
                    split_event.isin,
                    split_event.ex_date.date_naive()
                );
                add_stock_split_to_db(split_event).await?;
            }
        }
    }
    Ok(())
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
