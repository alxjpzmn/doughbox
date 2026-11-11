use anyhow::Context;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use super::BrokerClient;
use crate::services::trading::pies::PieRateLimited;

fn retry_after(headers: &reqwest::header::HeaderMap, fallback: u64) -> u64 {
    headers.get(reqwest::header::RETRY_AFTER).and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok()).unwrap_or(fallback).max(fallback)
}

impl BrokerClient {
    async fn account_get(&self, path: &str, fallback_wait: u64) -> anyhow::Result<Value> {
        let response = self.request(Method::GET, path).send().await
            .map_err(|_| anyhow::anyhow!("Trading 212 account state could not be read"))?;
        let status = response.status();
        if status == StatusCode::OK {
            return response.json().await.context("Trading 212 returned an invalid account response");
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(PieRateLimited { retry_after_seconds: retry_after(response.headers(), fallback_wait) }.into());
        }
        anyhow::bail!("Trading 212 account read failed (HTTP {})", status.as_u16());
    }

    pub async fn account_summary_value(&self) -> anyhow::Result<Value> {
        self.account_get("/equity/account/summary", 6).await
    }

    pub async fn positions_value(&self) -> anyhow::Result<Value> {
        self.account_get("/equity/positions", 2).await
    }
}
