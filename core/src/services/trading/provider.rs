use std::{borrow::Cow, fmt, str::FromStr, time::Duration};

use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Number, Value};

use crate::services::shared::{env::get_env_variable, secrets};

pub mod pies;
pub mod account;

#[cfg(test)]
static TEST_BROKER_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Dedicated, serialized test fixtures only; no production URL override exists.
#[cfg(test)]
pub(crate) fn set_test_broker_url(url: Option<String>) -> Result<()> {
    if let Some(value) = &url {
        let parsed = reqwest::Url::parse(value)?;
        if parsed.scheme() != "http" || parsed.host_str() != Some("127.0.0.1")
            || parsed.port().is_none() || parsed.path() != "/api/v0"
            || parsed.query().is_some() || parsed.fragment().is_some()
            || !parsed.username().is_empty() || parsed.password().is_some()
        {
            bail!("Test broker URL must be a loopback HTTP /api/v0 URL");
        }
    }
    *TEST_BROKER_URL.lock().map_err(|_| anyhow!("Test broker URL lock poisoned"))? = url;
    Ok(())
}

const CREDENTIALS_AAD: &[u8] = b"doughbox-trading212-v1";
const MAX_REJECTION_BODY: usize = 16 * 1024;
const MAX_REJECTION_FIELD_CHARS: usize = 400;

// Deliberately not Debug: neither credentials nor a client may be logged.
#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub api_key: String,
    pub api_secret: String,
}

pub fn check_master_key() -> Result<()> {
    master_key().map(|_| ())
}

pub fn encrypt_credentials(credentials: &Credentials) -> Result<String> {
    encrypt_with_key(credentials, &master_key()?)
}

pub fn decrypt_credentials(ciphertext: &str) -> Result<Credentials> {
    decrypt_with_key(ciphertext, &master_key()?)
}

fn master_key() -> Result<[u8; 32]> {
    let encoded = get_env_variable("TRADING_MASTER_KEY")
        .ok_or_else(|| anyhow!("TRADING_MASTER_KEY must be set to 64 hexadecimal characters"))?;
    secrets::parse_master_key(&encoded)
        .map_err(|_| anyhow!("TRADING_MASTER_KEY must contain exactly 64 hexadecimal characters"))
}

fn encrypt_with_key(credentials: &Credentials, key: &[u8; 32]) -> Result<String> {
    let plaintext = serde_json::to_vec(credentials)
        .map_err(|_| anyhow!("Trading credentials could not be encoded"))?;
    secrets::encrypt(&plaintext, key, CREDENTIALS_AAD)
}

fn decrypt_with_key(encoded: &str, key: &[u8; 32]) -> Result<Credentials> {
    let plaintext = secrets::decrypt(encoded, key, CREDENTIALS_AAD)?;
    serde_json::from_slice(&plaintext)
        .map_err(|_| anyhow!("Decrypted trading credentials are invalid"))
}

#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Instrument {
    pub ticker: String,
    pub name: String,
    pub currency: String,
    pub isin: String,
}

#[derive(Deserialize)]
struct BrokerInstrument {
    ticker: String,
    name: String,
    #[serde(rename = "currencyCode")]
    currency: String,
    isin: String,
    #[serde(rename = "type")]
    instrument_type: String,
}

#[derive(Deserialize)]
struct BrokerOrder {
    id: Value,
    status: String,
}

#[derive(Deserialize)]
struct HistoryItem {
    order: BrokerOrder,
}

#[derive(Deserialize)]
struct HistoryPage {
    #[serde(default)]
    items: Vec<HistoryItem>,
}

impl BrokerOrder {
    fn tracked(self) -> Option<(String, String)> {
        let id = identifier(&self.id)?;
        Some((id, canonical_order_status(&self.status).to_owned()))
    }
}

fn canonical_order_status(status: &str) -> &'static str {
    match status {
        "FILLED" => "filled",
        "PARTIALLY_FILLED" => "partial",
        "CANCELLED" | "REPLACED" => "cancelled",
        "REJECTED" => "rejected",
        _ => "open",
    }
}

pub struct BrokerClient {
    client: reqwest::Client,
    base_url: Cow<'static, str>,
    credentials: Credentials,
}

impl BrokerClient {
    pub fn new(environment: &str, credentials: Credentials) -> Result<Self> {
        let base_url = match environment {
            "demo" => "https://demo.trading212.com/api/v0",
            "live" => "https://live.trading212.com/api/v0",
            _ => bail!("Trading environment must be demo or live"),
        };
        #[cfg(test)]
        let test_url = TEST_BROKER_URL.lock()
            .map_err(|_| anyhow!("Test broker URL lock poisoned"))?.clone();
        #[cfg(test)]
        let base_url: Cow<'static, str> = test_url.clone().map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(base_url));
        #[cfg(not(test))]
        let base_url = Cow::Borrowed(base_url);
        #[cfg(test)]
        let https_only = test_url.is_none();
        #[cfg(not(test))]
        let https_only = true;
        if credentials.api_key.trim().is_empty() || credentials.api_secret.trim().is_empty() {
            bail!("Trading 212 API key and secret are required");
        }
        let client = reqwest::Client::builder()
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            // Protocol-level automatic retries are unsafe for order submissions too.
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("doughbox/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| anyhow!("Trading 212 HTTP client could not be initialized"))?;
        Ok(Self {
            client,
            base_url,
            credentials,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base_url))
            .basic_auth(
                &self.credentials.api_key,
                Some(&self.credentials.api_secret),
            )
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .request(reqwest::Method::GET, path)
            .send()
            .await
            .map_err(|_| anyhow!("Trading 212 could not be reached"))?;
        if !response.status().is_success() {
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                bail!(
                    "Trading 212 rejected the API key ID or Secret Key (HTTP {}). Nothing was saved. Check both values, the demo or live environment, and that the key can read the account and instruments.",
                    status.as_u16()
                );
            }
            bail!("Trading 212 request failed (HTTP {})", status.as_u16());
        }
        response
            .json()
            .await
            .map_err(|_| anyhow!("Trading 212 returned an invalid response"))
    }

    pub async fn account(&self) -> Result<Value> {
        #[derive(Deserialize)]
        struct Account {
            id: Value,
            currency: String,
        }
        let account: Account = self.get_json("/equity/account/summary").await?;
        let id = identifier(&account.id)
            .ok_or_else(|| anyhow!("Trading 212 returned an invalid account identifier"))?;
        if account.currency.trim().is_empty() {
            bail!("Trading 212 returned an invalid account currency");
        }
        // Never pass through arbitrary account fields from the broker.
        Ok(json!({ "id": id, "currency": account.currency }))
    }

    pub async fn instruments(&self) -> Result<Vec<Instrument>> {
        let instruments: Vec<BrokerInstrument> =
            self.get_json("/equity/metadata/instruments").await?;
        instruments
            .into_iter()
            .filter(|item| matches!(item.instrument_type.as_str(), "STOCK" | "ETF"))
            .map(|item| {
                if item.isin.trim().is_empty() {
                    bail!("Trading 212 returned an instrument without an ISIN");
                }
                Ok(Instrument {
                    ticker: item.ticker,
                    name: item.name,
                    currency: item.currency,
                    isin: item.isin,
                })
            })
            .collect()
    }

    pub async fn submit_limit(
        &self,
        ticker: &str,
        side: &str,
        quantity: Decimal,
        limit_price: Decimal,
        time_validity: &str,
    ) -> std::result::Result<String, SubmitError> {
        let payload = limit_payload(ticker, side, quantity, limit_price, time_validity)?;
        let mut response = self.post_limit(&payload).await?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            // 429 means this request was refused, so one later attempt cannot duplicate it.
            tokio::time::sleep(rate_limit_wait(response.headers())).await;
            response = self.post_limit(&payload).await?;
        }
        match submission_status(response.status()) {
            Ok(()) => {}
            Err(SubmitError::Rejected(mut message)) => {
                message.push_str(": ");
                message.push_str(&rejection_detail(response, &self.credentials).await);
                return Err(SubmitError::Rejected(message));
            }
            Err(unknown) => return Err(unknown),
        }
        let body: Value = response.json().await.map_err(|_| {
            SubmitError::Unknown(
                "Trading 212 returned an unreadable order confirmation; do not retry automatically"
                    .into(),
            )
        })?;
        submission_id(&body)
    }

    async fn post_limit(&self, payload: &Value) -> std::result::Result<reqwest::Response, SubmitError> {
        self.request(reqwest::Method::POST, "/equity/orders/limit")
            .json(payload)
            .send()
            .await
            .map_err(|_| SubmitError::Unknown(
                "Trading 212 order submission could not be confirmed; do not retry automatically".into(),
            ))
    }

    pub async fn pending_order_statuses(&self) -> Result<Vec<(String, String)>> {
        let orders: Vec<BrokerOrder> = self.get_json("/equity/orders").await?;
        Ok(orders.into_iter().filter_map(BrokerOrder::tracked).collect())
    }

    pub async fn historical_order_statuses(&self) -> Result<Vec<(String, String)>> {
        let page: HistoryPage = self.get_json("/equity/history/orders?limit=50").await?;
        Ok(page.items.into_iter().filter_map(|item| item.order.tracked()).collect())
    }
}

#[derive(Debug)]
pub enum SubmitError {
    Rejected(String),
    Unknown(String),
}

impl fmt::Display for SubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(message) | Self::Unknown(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SubmitError {}

fn limit_payload(
    ticker: &str,
    side: &str,
    quantity: Decimal,
    limit_price: Decimal,
    time_validity: &str,
) -> std::result::Result<Value, SubmitError> {
    if ticker.trim().is_empty() {
        return Err(SubmitError::Rejected(
            "An instrument ticker is required".into(),
        ));
    }
    if quantity <= Decimal::ZERO || limit_price <= Decimal::ZERO {
        return Err(SubmitError::Rejected(
            "Quantity and limit price must be positive".into(),
        ));
    }
    let signed_quantity = match side {
        "buy" => quantity,
        "sell" => -quantity,
        _ => {
            return Err(SubmitError::Rejected(
                "Trade side must be buy or sell".into(),
            ))
        }
    };
    if !matches!(time_validity, "DAY" | "GOOD_TILL_CANCEL") {
        return Err(SubmitError::Rejected(
            "Unsupported limit-order time validity".into(),
        ));
    }
    // Never use f64 or silently round an approved trade. arbitrary_precision keeps
    // the exact decimal digits as JSON numbers, as required by Trading 212.
    let number = |amount: Decimal| {
        Number::from_str(&amount.to_string())
            .map_err(|_| SubmitError::Rejected("Trade amount could not be encoded exactly".into()))
    };
    Ok(json!({
        "ticker": ticker,
        "quantity": number(signed_quantity)?,
        "limitPrice": number(limit_price)?,
        "timeValidity": time_validity,
    }))
}

fn rate_limit_wait(headers: &reqwest::header::HeaderMap) -> std::time::Duration {
    let seconds = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3)
        .clamp(3, 30);
    std::time::Duration::from_secs(seconds)
}

fn submission_status(status: StatusCode) -> std::result::Result<(), SubmitError> {
    if status.is_success() {
        return Ok(());
    }
    if status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT {
        // Includes 429: the rate-limited request was rejected, not accepted.
        return Err(SubmitError::Rejected(format!(
            "Trading 212 rejected the order (HTTP {})",
            status.as_u16()
        )));
    }
    Err(SubmitError::Unknown(format!(
        "Trading 212 order outcome is uncertain (HTTP {}); do not retry automatically",
        status.as_u16()
    )))
}

/// Only allowlisted JSON rejection fields reach saved proposals. Failure to read
/// diagnostics must never turn an already definite HTTP rejection into uncertainty.
async fn rejection_detail(
    mut response: reqwest::Response,
    credentials: &Credentials,
) -> Cow<'static, str> {
    if response.content_length().is_some_and(|length| length > MAX_REJECTION_BODY as u64) {
        return Cow::Borrowed("Broker rejection details exceeded the 16 KiB safety limit; raw response omitted.");
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return Cow::Borrowed("Broker rejection details could not be fully read; the HTTP rejection is still definite."),
        };
        if chunk.len() > MAX_REJECTION_BODY - bytes.len() {
            return Cow::Borrowed("Broker rejection details exceeded the 16 KiB safety limit; raw response omitted.");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Cow::Borrowed("The broker supplied no rejection reason (empty response body).");
    }
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return Cow::Borrowed("Broker rejection details were not valid JSON; raw response omitted."),
    };
    let Some(object) = body.as_object() else {
        return Cow::Borrowed("Broker rejection JSON was not an object; raw response omitted.");
    };
    let encoded_credentials = BASE64.encode(format!("{}:{}", credentials.api_key, credentials.api_secret));
    let mut secrets = [
        credentials.api_key.as_str(), credentials.api_secret.as_str(), encoded_credentials.as_str(),
    ];
    // Redact longer overlapping secrets first, before truncation can expose a prefix.
    secrets.sort_unstable_by_key(|secret| std::cmp::Reverse(secret.len()));
    let mut details = Vec::with_capacity(6);
    for field in ["title", "detail", "type", "code", "message", "clarification", "error"] {
        let Some(text) = object.get(field).and_then(Value::as_str) else { continue; };
        if text.trim().is_empty() { continue; }
        let mut text = text.to_owned();
        for &secret in secrets.iter().filter(|secret| !secret.is_empty()) {
            text = text.replace(secret, "[redacted]");
        }
        let text: String = text.trim().chars()
            .map(|character| if character.is_control() { ' ' } else { character })
            .take(MAX_REJECTION_FIELD_CHARS)
            .collect();
        if !text.trim().is_empty() && !details.contains(&text) {
            details.push(text);
        }
    }
    if details.is_empty() {
        Cow::Borrowed("Broker JSON supplied no readable title, detail, type, code, message or clarification; raw response omitted.")
    } else {
        Cow::Owned(details.join("; "))
    }
}

fn identifier(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn submission_id(body: &Value) -> std::result::Result<String, SubmitError> {
    let id = body.get("id").and_then(identifier).ok_or_else(|| {
        SubmitError::Unknown(
            "Trading 212 did not return an order identifier; do not retry automatically".into(),
        )
    })?;
    if body.get("status").and_then(Value::as_str) == Some("REJECTED") {
        return Err(SubmitError::Rejected(
            "Trading 212 rejected the order".into(),
        ));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chacha20poly1305::{
        aead::{Aead, KeyInit, Payload},
        XChaCha20Poly1305, XNonce,
    };
    use crate::services::shared::secrets::encode_hex;
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

    struct BrokerFixture {
        broker: BrokerClient,
        calls: Arc<AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for BrokerFixture {
        fn drop(&mut self) { self.server.abort(); }
    }

    async fn broker_fixture(status: StatusCode, body: String) -> BrokerFixture {
        use axum::{routing::post, Router};
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let app = Router::new().route("/api/v0/equity/orders/limit", post(move || {
            let body = body.clone();
            let received = received.clone();
            async move {
                received.fetch_add(1, Ordering::SeqCst);
                let body = axum::body::Body::from_stream(futures::stream::once(
                    async move { Ok::<_, std::io::Error>(body) }
                ));
                (status, [("content-type", "application/json")], body)
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = Cow::Owned(format!("http://{}/api/v0", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        BrokerFixture {
            broker: BrokerClient {
                client: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .timeout(Duration::from_secs(2))
                    .build().unwrap(),
                base_url,
                credentials: Credentials { api_key: "fixture-key".into(), api_secret: "fixture-secret".into() },
            },
            calls, server,
        }
    }

    #[tokio::test]
    async fn broker_validation_reason_is_preserved_without_resubmitting() {
        let fixture = broker_fixture(StatusCode::BAD_REQUEST, json!({
            "code":"ValidationError",
            "clarification":"Quantity precision must be limited to 3 decimal places"
        }).to_string()).await;
        let error = fixture.broker.submit_limit(
            "SANe_EQ", "buy", Decimal::from_str_exact("19.8373").unwrap(),
            Decimal::from_str_exact("10.082").unwrap(), "DAY"
        ).await.unwrap_err();
        let SubmitError::Rejected(message) = error else { panic!("HTTP 400 is a definite rejection"); };
        assert!(message.contains("Quantity precision must be limited to 3 decimal places"), "{message}");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejection_details_are_allowlisted_bounded_and_redact_credentials() {
        let encoded = BASE64.encode("fixture-key:fixture-secret");
        let fixture = broker_fixture(StatusCode::BAD_REQUEST, json!({
            "code":"ValidationError",
            "message":format!("fixture-key\nfixture-secret Basic {encoded}\u{001b} {}", "x".repeat(1000)),
            "clarification":"Quantity precision must be limited to 3 decimal places",
            "request":{"authorization":"must-not-be-saved"},
            "stackTrace":"must-not-be-saved"
        }).to_string()).await;
        let error = fixture.broker.submit_limit("SANe_EQ", "buy", Decimal::ONE, Decimal::ONE, "DAY")
            .await.unwrap_err();
        let SubmitError::Rejected(message) = error else { panic!("Expected definite rejection"); };
        for secret in ["fixture-key", "fixture-secret", encoded.as_str(), "must-not-be-saved"] {
            assert!(!message.contains(secret), "{message}");
        }
        assert!(message.contains("[redacted]"));
        assert!(message.contains("Quantity precision must be limited to 3 decimal places"));
        assert!(!message.chars().any(char::is_control));
        assert!(message.chars().count() <= 1300);
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn problem_details_rejection_keeps_title_detail_and_type_only() {
        let fixture = broker_fixture(StatusCode::BAD_REQUEST, json!({
            "type":"/api-errors/instrument-shares",
            "title":"Bad Request",
            "status":400,
            "detail":"instrumentShares must sum to 1; fixture-key was not accepted",
            "instance":"/logs/untrusted-response",
            "request":{"authorization":"fixture-secret"}
        }).to_string()).await;
        let error = fixture.broker.submit_limit("SANe_EQ", "buy", Decimal::ONE, Decimal::ONE, "DAY").await.unwrap_err();
        let SubmitError::Rejected(message) = error else { panic!("problem details remain a definite rejection") };
        assert!(message.contains("Bad Request"), "{message}");
        assert!(message.contains("instrumentShares must sum to 1"), "{message}");
        assert!(message.contains("/api-errors/instrument-shares"), "{message}");
        assert!(message.contains("[redacted]"), "{message}");
        assert!(!message.contains("fixture-key") && !message.contains("fixture-secret"), "{message}");
        assert!(!message.contains("untrusted-response"), "{message}");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn malformed_empty_and_oversized_rejection_bodies_remain_definite_failures() {
        for body in [
            String::new(),
            "{broken".into(),
            "<html>untrusted-response</html>".into(),
            json!({"code":"untrusted-response", "clarification":"x".repeat(MAX_REJECTION_BODY)}).to_string(),
            json!({"code":123, "message":{}, "request":"untrusted-response"}).to_string(),
        ] {
            let fixture = broker_fixture(StatusCode::BAD_REQUEST, body).await;
            let error = fixture.broker.submit_limit("SANe_EQ", "buy", Decimal::ONE, Decimal::ONE, "DAY")
                .await.unwrap_err();
            let SubmitError::Rejected(message) = error else { panic!("Bad details must not change a known rejection"); };
            assert!(message.contains("HTTP 400"), "{message}");
            assert!(!message.contains("untrusted-response"));
            assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn interrupted_rejection_body_does_not_make_the_order_outcome_unknown() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = Cow::Owned(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 512];
            while !request.ends_with(b"}") {
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
            }
            socket.write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{"
            ).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let broker = BrokerClient {
            client: reqwest::Client::builder().timeout(Duration::from_secs(2))
                .retry(reqwest::retry::never()).build().unwrap(),
            base_url,
            credentials: Credentials { api_key: "fixture-key".into(), api_secret: "fixture-secret".into() },
        };
        let error = broker.submit_limit("SANe_EQ", "buy", Decimal::ONE, Decimal::ONE, "DAY")
            .await.unwrap_err();
        server.await.unwrap();
        let SubmitError::Rejected(message) = error else { panic!("HTTP 400 remains definite when reading its body fails"); };
        assert!(message.contains("HTTP 400"), "{message}");
    }

    #[tokio::test]
    async fn rejection_shaped_bodies_do_not_make_uncertain_http_outcomes_retryable() {
        for status in [StatusCode::REQUEST_TIMEOUT, StatusCode::INTERNAL_SERVER_ERROR, StatusCode::FOUND] {
            let fixture = broker_fixture(status, json!({
                "code":"ValidationError", "clarification":"untrusted-rejection"
            }).to_string()).await;
            let error = fixture.broker.submit_limit("SANe_EQ", "buy", Decimal::ONE, Decimal::ONE, "DAY")
                .await.unwrap_err();
            let SubmitError::Unknown(message) = error else { panic!("Transport/status uncertainty must remain unknown"); };
            assert!(!message.contains("untrusted-rejection"));
            assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn credentials_are_randomized_and_authenticated() {
        let credentials = Credentials {
            api_key: "test-api-key".into(),
            api_secret: "test-api-secret".into(),
        };
        let key = [7; 32];
        let first = encrypt_with_key(&credentials, &key).unwrap();
        let second = encrypt_with_key(&credentials, &key).unwrap();
        assert_ne!(first, second);
        assert!(!first.contains(&credentials.api_key));
        assert!(!first.contains(&credentials.api_secret));
        let decrypted = decrypt_with_key(&first, &key).unwrap();
        assert_eq!(decrypted.api_key, credentials.api_key);
        assert_eq!(decrypted.api_secret, credentials.api_secret);
        assert!(decrypt_with_key(&first, &[8; 32]).is_err());

        let mut changed = first.into_bytes();
        changed[0] = if changed[0] == b'0' { b'1' } else { b'0' };
        assert!(decrypt_with_key(std::str::from_utf8(&changed).unwrap(), &key).is_err());

        let cipher = XChaCha20Poly1305::new((&key).into());
        let nonce = [0; 24];
        let wrong_aad = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: br#"{"api_key":"test","api_secret":"test"}"#,
                    aad: b"another-purpose",
                },
            )
            .unwrap();
        let encoded = encode_hex(nonce.iter().chain(wrong_aad.iter()).copied());
        assert!(decrypt_with_key(&encoded, &key).is_err());
    }

    #[test]
    fn invalid_ciphertext_and_master_keys_are_rejected_without_panics() {
        for encoded in ["", "0", "ff", "zz", &"0".repeat(79), &"g".repeat(80)] {
            assert!(decrypt_with_key(encoded, &[7; 32]).is_err());
        }
        for encoded in ["", "key", &"0".repeat(63), &"0".repeat(65), &"g".repeat(64)] {
            assert!(secrets::parse_master_key(encoded).is_err());
        }
        assert_eq!(secrets::parse_master_key(&"aB".repeat(32)).unwrap(), [0xab; 32]);
    }

    #[test]
    fn trading_envelopes_remain_compatible_with_the_original_byte_layout() {
        let key = [7; 32];
        let cipher = XChaCha20Poly1305::new((&key).into());
        let nonce = [0x42; 24];
        let plaintext = br#"{"api_key":"legacy-key","api_secret":"legacy-secret"}"#;
        let legacy_ciphertext = cipher.encrypt(
            XNonce::from_slice(&nonce),
            Payload { msg: plaintext, aad: b"doughbox-trading212-v1" },
        ).unwrap();
        let legacy_envelope = encode_hex(nonce.iter().chain(legacy_ciphertext.iter()).copied());
        let credentials = decrypt_with_key(&legacy_envelope, &key).unwrap();
        assert_eq!(credentials.api_key, "legacy-key");
        assert_eq!(credentials.api_secret, "legacy-secret");

        let envelope = encrypt_with_key(&credentials, &key).unwrap();
        let mut bytes = vec![0; envelope.len() / 2];
        assert!(secrets::decode_hex_into(&envelope, &mut bytes));
        let decoded = cipher.decrypt(
            XNonce::from_slice(&bytes[..24]),
            Payload { msg: &bytes[24..], aad: b"doughbox-trading212-v1" },
        ).unwrap();
        assert_eq!(decoded, plaintext);
    }

    #[test]
    fn trade_amounts_preserve_every_decimal_digit_and_sell_sign() {
        let quantity = Decimal::from_str("0.1234567890123456789012345678").unwrap();
        let price = Decimal::from_str("79228162514264337593543950335").unwrap();
        let body =
            limit_payload("AAPL_US_EQ", "sell", quantity, price, "GOOD_TILL_CANCEL").unwrap();
        assert!(body["quantity"].is_number());
        assert!(body["limitPrice"].is_number());
        assert_eq!(
            body["quantity"].to_string(),
            "-0.1234567890123456789012345678"
        );
        assert_eq!(
            body["limitPrice"].to_string(),
            "79228162514264337593543950335"
        );
        assert_eq!(body["timeValidity"], "GOOD_TILL_CANCEL");
        let encoded = serde_json::to_string(&body).unwrap();
        let decoded: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, body);
        let buy = limit_payload("AAPL_US_EQ", "buy", Decimal::ONE, Decimal::ONE, "DAY").unwrap();
        assert_eq!(buy["quantity"].to_string(), "1");
    }

    #[test]
    fn invalid_orders_are_rejected_before_submission() {
        for (side, quantity, price, validity) in [
            ("buy", Decimal::ZERO, Decimal::ONE, "DAY"),
            ("sell", -Decimal::ONE, Decimal::ONE, "DAY"),
            ("buy", Decimal::ONE, Decimal::ZERO, "DAY"),
            ("buy", Decimal::ONE, -Decimal::ONE, "DAY"),
            ("other", Decimal::ONE, Decimal::ONE, "DAY"),
            ("buy", Decimal::ONE, Decimal::ONE, "GTC"),
        ] {
            assert!(matches!(
                limit_payload("AAPL_US_EQ", side, quantity, price, validity),
                Err(SubmitError::Rejected(_))
            ));
        }
    }

    #[test]
    fn submission_status_preserves_uncertainty() {
        for code in [400, 401, 403, 404, 409, 422, 429] {
            assert!(matches!(
                submission_status(StatusCode::from_u16(code).unwrap()),
                Err(SubmitError::Rejected(_))
            ));
        }
        for code in [301, 307, 408, 500, 502, 503, 504] {
            assert!(matches!(
                submission_status(StatusCode::from_u16(code).unwrap()),
                Err(SubmitError::Unknown(_))
            ));
        }
        assert!(submission_status(StatusCode::CREATED).is_ok());
    }

    #[test]
    fn order_confirmation_requires_an_identifier_and_preserves_large_ids() {
        let body: Value = serde_json::from_str(r#"{"id":123456789012345678901234567890}"#).unwrap();
        assert_eq!(
            submission_id(&body).unwrap(),
            "123456789012345678901234567890"
        );
        assert_eq!(
            submission_id(&json!({"id": "order-123"})).unwrap(),
            "order-123"
        );
        for body in [
            json!({}),
            json!({"id": null}),
            json!({"id": " "}),
            json!({"id": {}}),
        ] {
            assert!(matches!(submission_id(&body), Err(SubmitError::Unknown(_))));
        }
        assert!(matches!(
            submission_id(&json!({"id": 123, "status": "REJECTED"})),
            Err(SubmitError::Rejected(_))
        ));
    }
}
