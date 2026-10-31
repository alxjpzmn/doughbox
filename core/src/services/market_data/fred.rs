use chrono::NaiveDate;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::services::shared::env::get_env_variable;

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
    fred_data_set
        .observations
        .iter()
        .filter_map(|item| {
            let observation_date = NaiveDate::parse_from_str(&item.date, "%Y-%m-%d").ok()?;
            let value = item.value.parse::<Decimal>().ok()?;
            (observation_date <= date).then_some((observation_date, value))
        })
        .max_by_key(|(observation_date, _)| *observation_date)
        .map(|(_, value)| value)
        .ok_or_else(|| anyhow::anyhow!("No valid FRED observation on or before {date}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn dataset(items: &[(&str, &str)]) -> FREDResponse {
        FREDResponse {
            observations: items
                .iter()
                .map(|(date, value)| FREDResponseItem {
                    date: (*date).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn uses_latest_valid_observation_without_looking_ahead() {
        let data = dataset(&[
            ("2025-01-07", "110"),
            ("2025-01-03", "100"),
            ("2025-01-06", "."),
            ("2025-01-02", "90"),
            ("invalid-date", "999"),
        ]);
        let requested = NaiveDate::from_ymd_opt(2025, 1, 6).unwrap();
        assert_eq!(get_fred_value_for_date(&data, requested).await.unwrap(), dec!(100));
        let requested = NaiveDate::from_ymd_opt(2025, 1, 7).unwrap();
        assert_eq!(get_fred_value_for_date(&data, requested).await.unwrap(), dec!(110));
    }

    #[tokio::test]
    async fn errors_when_no_valid_prior_observation_exists() {
        let requested = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        for data in [
            dataset(&[]),
            dataset(&[("2025-01-02", "100")]),
            dataset(&[("2024-12-31", ".")]),
        ] {
            assert!(get_fred_value_for_date(&data, requested).await.is_err());
        }
    }
}
