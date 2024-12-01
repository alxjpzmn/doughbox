use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::services::env::get_env_variable;

#[derive(Deserialize, Debug)]
pub struct FREDResponse {
    pub observations: Vec<FREDResponseItem>,
}
#[derive(Deserialize, Debug)]
pub struct FREDResponseItem {
    pub value: String,
    date: String,
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
