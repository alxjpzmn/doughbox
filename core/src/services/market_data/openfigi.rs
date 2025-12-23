use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use tokio::time::sleep;

use crate::{
    database::queries::ticker_conversion::{insert_ticker_conversion, query_symbol_from_isin},
    services::parsers::remove_first_and_last,
};

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

    println!("Looking for symbol of ISIN {} in the database...", isin);
    // First, we check whether the have a symbol for that ISIN already in the DB
    let symbol_in_db = query_symbol_from_isin(isin).await?;

    if &symbol_in_db == "Unidentified" {
        println!("Getting symbol for ISIN {} from OpenFIGI", &isin);
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

        // Check for specific error responses
        if open_figi_mapping_response == r#"[{"warning":"No identifier found."}]"# {
            return Ok("NONE_FOUND".to_string());
        }
        if open_figi_mapping_response == r#"[{"error":"Invalid idValue format"}]"# {
            return Ok("NONE_FOUND".to_string());
        }

        // Clean and parse the response
        let cleaned_response = remove_first_and_last(&open_figi_mapping_response);
        let response = serde_json::from_str::<OpenFIGIResponse>(cleaned_response).context(
            format!("Failed to deserialize OpenFIGI response for ISIN {}", isin),
        )?;

        // Check if we have data in the response
        if response.data.is_empty() {
            return Ok("NONE_FOUND".to_string());
        }

        // Get the ticker from the first data entry
        let ticker = response.data[0].ticker.as_str();

        // Handle empty ticker
        if ticker.is_empty() {
            return Ok("NONE_FOUND".to_string());
        }

        // OpenFIGI API is rate limited to 5 requests / minute for unregistered users
        sleep(Duration::from_millis(12000)).await;

        // We store the found ticker conversion in the database
        insert_ticker_conversion(isin, ticker).await?;

        Ok(ticker.to_string())
    } else {
        Ok(symbol_in_db.to_string())
    }
}
