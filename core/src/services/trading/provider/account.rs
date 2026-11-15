use anyhow::Context;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use super::BrokerClient;
use crate::services::trading::rate_limits::BrokerRateLimited;

impl BrokerClient {
    async fn account_get(&self, path: &str, bucket: &str) -> anyhow::Result<Value> {
        let request = self.request(Method::GET, path);
        self.admit(bucket, true).await?;
        self.authorize(true).await?;
        let response = request.send().await
            .map_err(|_| anyhow::anyhow!("Trading 212 account state could not be read"))?;
        let observed = self.observe(bucket, response.status(), response.headers()).await;
        let authorized = self.authorize(true).await;
        let retry_after_seconds = observed?;
        authorized?;
        let status = response.status();
        if status == StatusCode::OK {
            let value = response.json().await.context("Trading 212 returned an invalid account response")?;
            self.authorize(true).await?;
            return Ok(value);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(BrokerRateLimited { retry_after_seconds }.into());
        }
        anyhow::bail!("Trading 212 account read failed (HTTP {})", status.as_u16());
    }

    pub async fn account_summary_value(&self) -> anyhow::Result<Value> {
        self.account_get("/equity/account/summary", "account_summary").await
    }

    pub async fn positions_value(&self) -> anyhow::Result<Value> {
        self.account_get("/equity/positions", "positions").await
    }
}
