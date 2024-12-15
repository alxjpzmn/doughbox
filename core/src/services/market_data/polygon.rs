use std::{println, time::Duration};

use chrono::{DateTime, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::time::sleep;

use crate::services::{
    instruments::stock_splits::StockSplit,
    parsers::parse_timestamp,
    shared::{env::get_env_variable, util::hash_string},
};

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
