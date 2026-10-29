use anyhow::Context;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::time::Duration;
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
    let polygon_key = get_env_variable("POLYGON_TOKEN")
        .filter(|token| !token.trim().is_empty())
        .context("POLYGON_TOKEN is required for stock-split housekeeping")?;
    // Polygon API is rate limited to 5 requests / minute for free users.
    sleep(Duration::from_millis(12000)).await;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let polygon_response_data = fetch_splits(
        &client,
        "https://api.polygon.io/v3/reference/splits",
        symbol,
        &polygon_key,
    )
    .await?;

    let mut splits_found = vec![];

    for polygon_response_item in polygon_response_data.results {
        let stock_split_information = StockSplit {
            id: hash_string(format!("{}{}", isin, polygon_response_item.execution_date).as_str()),
            ex_date: parse_timestamp(
                format!("{} 16:00:00", &polygon_response_item.execution_date).as_str(),
            )
            .map_err(|_| anyhow::anyhow!("Polygon returned an invalid split execution date"))?,
            from_factor: polygon_response_item.split_from,
            to_factor: polygon_response_item.split_to,
            isin: isin.to_string(),
        };
        splits_found.push(stock_split_information)
    }

    Ok(splits_found)
}

async fn fetch_splits(
    client: &Client,
    endpoint: &str,
    symbol: &str,
    token: &str,
) -> anyhow::Result<PolygonSplitResponse> {
    // Do not attach reqwest errors as sources: their URLs include apiKey.
    // Neither response bodies nor upstream error messages are safe to expose.
    let response = client
        .get(endpoint)
        .query(&[("ticker", symbol), ("apiKey", token)])
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Polygon stock-split request failed"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "Polygon stock-split request returned HTTP {}",
        response.status().as_u16()
    );
    response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("Polygon returned invalid split data"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn upstream_errors_never_expose_polygon_token() {
        for (status, body) in [
            ("401 Unauthorized", r#"{"error":"secret-polygon-token"}"#),
            ("200 OK", r#"{"unexpected":"secret-polygon-token"}"#),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/splits", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let mut received = 0;
                while !request[..received].ends_with(b"\r\n\r\n") {
                    let count = socket.read(&mut request[received..]).await.unwrap();
                    assert!(count > 0, "incomplete HTTP request headers");
                    received += count;
                }
                socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let error = fetch_splits(&Client::new(), &url, "AAPL", "secret-polygon-token")
                .await
                .unwrap_err();
            assert!(!format!("{error:#?}").contains("secret-polygon-token"));
            server.await.unwrap();
        }
        let error = fetch_splits(
            &Client::new(),
            "http://[invalid",
            "AAPL",
            "secret-polygon-token",
        )
        .await
        .unwrap_err();
        assert!(!format!("{error:#?}").contains("secret-polygon-token"));
    }
}
