use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use tokio::time::sleep;

use crate::services::parsers::remove_first_and_last;

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

    let open_figi_mapping_response_data = serde_json::from_str::<OpenFIGIResponse>(
        remove_first_and_last(&open_figi_mapping_response),
    )
    .unwrap();

    // OpenFIGI API is rate limited to 5 requests / minute for unregistered users
    sleep(Duration::from_millis(12000)).await;

    Ok(open_figi_mapping_response_data.data[0].ticker.to_string())
}
