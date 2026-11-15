use std::fmt;

use anyhow::{anyhow, Result};
use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};

use super::BrokerClient;
use crate::services::trading::rate_limits::BrokerRateLimited;

pub(crate) mod download;

const EXPORT_PATH: &str = "/equity/history/exports";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDataIncluded {
    pub include_orders: bool,
    pub include_dividends: bool,
    pub include_interest: bool,
    pub include_transactions: bool,
}

impl HistoryDataIncluded {
    pub fn all() -> Self {
        Self {
            include_orders: true,
            include_dividends: true,
            include_interest: true,
            include_transactions: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum HistoryReportStatus {
    Queued,
    Processing,
    Running,
    Canceled,
    Failed,
    Finished,
}

// Deliberately not Debug or Serialize: download links are signed credentials.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryReport {
    #[serde(deserialize_with = "report_id")]
    pub report_id: String,
    pub time_from: DateTime<Utc>,
    pub time_to: DateTime<Utc>,
    pub data_included: HistoryDataIncluded,
    pub status: HistoryReportStatus,
    pub download_link: Option<String>,
}

impl HistoryReport {
    pub fn matches(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        normalized_time(self.time_from) == normalized_time(from)
            && normalized_time(self.time_to) == normalized_time(to)
            && self.data_included == HistoryDataIncluded::all()
    }
}

fn normalized_time(time: DateTime<Utc>) -> DateTime<Utc> {
    // PostgreSQL and the shared financial importer use microsecond precision.
    time.with_nanosecond(time.nanosecond() / 1_000 * 1_000)
        .expect("truncating nanoseconds preserves a valid timestamp")
}

fn report_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    i64::deserialize(deserializer).map(|id| id.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedReport {
    #[serde(deserialize_with = "report_id")]
    report_id: String,
}

/// Only safe, locally authored diagnostics belong in persisted sync errors.
#[derive(Debug)]
pub struct HistoryHttpError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for HistoryHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HistoryHttpError {}

impl HistoryHttpError {
    fn response(status: StatusCode) -> Self {
        let (code, explanation) = match status {
            StatusCode::BAD_REQUEST => (
                "bad_filtering_arguments",
                "Trading 212 rejected the history report filtering arguments.",
            ),
            StatusCode::UNAUTHORIZED => (
                "invalid_credentials",
                "Trading 212 rejected the API credentials. Check the connected account's API key and secret.",
            ),
            StatusCode::FORBIDDEN => (
                "export_unavailable",
                "Trading 212 refused the history request (HTTP 403). The response did not say history access is missing.",
            ),
            StatusCode::REQUEST_TIMEOUT => (
                "request_timeout",
                "Trading 212 timed out while handling the history request.",
            ),
            _ if status.is_server_error() => (
                "broker_unavailable",
                "Trading 212 could not complete the history request.",
            ),
            _ => ("http_error", "Trading 212 rejected the history request."),
        };
        Self {
            status: status.as_u16(),
            code,
            message: format!("{explanation} (HTTP {})", status.as_u16()),
        }
    }

    pub(crate) fn forbidden(body: &[u8]) -> Self {
        let text = String::from_utf8_lossy(&body[..body.len().min(4096)]);
        let lower = text.to_ascii_lowercase();
        if lower.contains("not-available-in-demo-account") {
            return Self {
                status: 403,
                code: "export_unavailable_demo",
                message: "Trading 212 does not offer CSV history export on this demo account. History access is still used. (HTTP 403)".into(),
            };
        }
        if let Some(scope) = missing_scope(&text) {
            return Self {
                status: 403,
                code: "missing_scope",
                message: format!("Trading 212 rejected the request because scope {scope} is missing for this API key. (HTTP 403)"),
            };
        }
        if lower.contains("<html") || lower.contains("access denied") || lower.contains("cloudflare") {
            return Self {
                status: 403,
                code: "broker_edge_blocked",
                message: "Trading 212's edge blocked the request before API permissions were checked. This is not a missing history permission. (HTTP 403)".into(),
            };
        }
        Self::response(StatusCode::FORBIDDEN)
    }

    /// CSV export can be refused while the history list scopes are present.
    /// A named missing history-list scope is the only 403 that should stop that fallback.
    pub(crate) fn allows_history_list_fallback(&self) -> bool {
        self.status == 403 && !self.denies_history_lists()
    }

    pub(crate) fn denies_history_lists(&self) -> bool {
        self.code == "missing_scope" && ["history:orders", "history:dividends", "history:transactions"]
            .iter().any(|scope| self.message.contains(scope))
    }

    fn invalid_schema(status: StatusCode) -> Self {
        Self {
            status: status.as_u16(),
            code: "invalid_schema",
            message: "Trading 212 returned an invalid history report response. No report coverage was accepted.".into(),
        }
    }
}

fn missing_scope(body: &str) -> Option<String> {
    let start = body.find("Scope(")? + "Scope(".len();
    let end = body[start..].find(") missing")?;
    let scope = &body[start..start + end];
    if (1..=80).contains(&scope.len()) && scope.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.' | b'-')) {
        Some(scope.to_owned())
    } else {
        None
    }
}

pub struct HistoryActivityPage {
    pub items: Vec<serde_json::Value>,
    pub next_page_path: Option<String>,
}
impl BrokerClient {
    pub async fn request_history_report(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Request {
            data_included: HistoryDataIncluded,
            time_from: String,
            time_to: String,
        }
        let request = self.request(Method::POST, EXPORT_PATH).json(&Request {
            data_included: HistoryDataIncluded::all(),
            time_from: normalized_time(from).to_rfc3339_opts(SecondsFormat::Micros, true),
            time_to: normalized_time(to).to_rfc3339_opts(SecondsFormat::Micros, true),
        });
        let response = self.history_response(request, "export_create").await?;
        let report: CreatedReport = self.history_json(response).await?;
        Ok(report.report_id)
    }

    pub async fn history_reports(&self) -> Result<Vec<HistoryReport>> {
        let request = self.request(Method::GET, EXPORT_PATH);
        let response = self.history_response(request, "export_status").await?;
        self.history_json(response).await
    }

    pub async fn history_activity_page(&self, path: &str) -> Result<HistoryActivityPage> {
        let base = path.split_once('?').map_or(path, |(base, _)| base);
        if !matches!(base, "/equity/history/orders" | "/equity/history/dividends" | "/equity/history/transactions") || path.contains("//") {
            anyhow::bail!("Trading 212 history page is outside the history API");
        }
        let request = self.request(Method::GET, path);
        let response = self.history_response(request, "order_history").await?;
        let page: serde_json::Value = self.history_json(response).await?;
        let items = page.get("items").and_then(serde_json::Value::as_array).cloned()
            .ok_or_else(|| HistoryHttpError::invalid_schema(StatusCode::OK))?;
        let next_page_path = page.get("nextPagePath").and_then(serde_json::Value::as_str).map(str::to_owned);
        Ok(HistoryActivityPage { items, next_page_path })
    }


    async fn history_response(&self, request: RequestBuilder, bucket: &str) -> Result<Response> {
        // Export creation is financially read-only, despite its POST method.
        self.admit(bucket, true).await?;
        self.authorize(true).await?;
        let response = request.send().await
            .map_err(|_| anyhow!("Trading 212 history service could not be reached. The report request may not have been acknowledged."))?;
        // Observe even a failed response or a response to a revoked connection.
        let observed = self.observe(bucket, response.status(), response.headers()).await;
        let authorized = self.authorize(true).await;
        let retry_after_seconds = observed?;
        authorized?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(BrokerRateLimited { retry_after_seconds }.into());
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.bytes().await.unwrap_or_default();
            if status == StatusCode::FORBIDDEN {
                return Err(HistoryHttpError::forbidden(&body).into());
            }
            return Err(HistoryHttpError::response(status).into());
        }
        Ok(response)
    }

    async fn history_json<T: DeserializeOwned>(&self, response: Response) -> Result<T> {
        let status = response.status();
        // Separate interrupted transport from a complete but invalid schema.
        // Never retain reqwest/serde errors, which may expose URLs or payloads.
        let body = response.bytes().await
            .map_err(|_| anyhow!("Trading 212 history response could not be read completely."))?;
        let value = serde_json::from_slice(&body)
            .map_err(|_| HistoryHttpError::invalid_schema(status))?;
        self.authorize(true).await?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, sync::{Arc, Mutex}, time::Duration};

    use axum::{body::{to_bytes, Body}, extract::{Request, State}, http::HeaderMap, response::IntoResponse, Router};
    use serde_json::{json, Value};
    use tokio::task::JoinHandle;

    use super::*;
    use crate::services::trading::provider::Credentials;

    struct CapturedRequest {
        method: Method,
        path: String,
        body: Vec<u8>,
    }

    struct FixtureState {
        status: StatusCode,
        headers: HeaderMap,
        body: String,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    struct Fixture {
        broker: BrokerClient,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        server: JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    async fn fixture(status: StatusCode, headers: HeaderMap, body: String) -> Fixture {
        async fn reply(State(state): State<Arc<FixtureState>>, request: Request) -> axum::response::Response {
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, 16 * 1024).await.unwrap().to_vec();
            state.requests.lock().unwrap().push(CapturedRequest {
                method: parts.method,
                path: parts.uri.path().to_owned(),
                body,
            });
            (state.status, state.headers.clone(), Body::from(state.body.clone())).into_response()
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().fallback(reply).with_state(Arc::new(FixtureState {
            status, headers, body, requests: requests.clone(),
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = Cow::Owned(format!("http://{}/api/v0", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fixture {
            broker: BrokerClient::transport_only(
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .timeout(Duration::from_secs(2))
                    .build().unwrap(),
                base_url,
                Credentials { api_key: "history-fixture-key".into(), api_secret: "history-fixture-secret".into() },
            ),
            requests,
            server,
        }
    }

    fn bounds() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            "2025-04-01T02:00:00.123456789+02:00".parse().unwrap(),
            "2025-04-02T00:00:00Z".parse().unwrap(),
        )
    }

    fn report() -> Value {
        json!({
            "reportId": 9223372036854775807_i64,
            "timeFrom": "2025-04-01T00:00:00.123456Z",
            "timeTo": "2025-04-02T00:00:00Z",
            "dataIncluded": {
                "includeOrders": true,
                "includeDividends": true,
                "includeInterest": true,
                "includeTransactions": true
            },
            "status": "Finished",
            "downloadLink": "https://reports.example/export.csv?signature=do-not-persist"
        })
    }

    #[tokio::test]
    async fn export_request_is_exact_and_preserves_int64_identity() {
        let fixture = fixture(StatusCode::OK, HeaderMap::new(), json!({"reportId": 9223372036854775807_i64}).to_string()).await;
        let (from, to) = bounds();
        let id = fixture.broker.request_history_report(from, to).await.unwrap();
        assert_eq!(id, "9223372036854775807");
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/api/v0/equity/history/exports");
        assert_eq!(serde_json::from_slice::<Value>(&requests[0].body).unwrap(), json!({
            "dataIncluded": {
                "includeOrders": true, "includeDividends": true,
                "includeInterest": true, "includeTransactions": true
            },
            "timeFrom": "2025-04-01T00:00:00.123456Z",
            "timeTo": "2025-04-02T00:00:00.000000Z"
        }));
    }

    #[tokio::test]
    async fn status_reports_preserve_identity_and_require_exact_coverage() {
        let fixture = fixture(StatusCode::OK, HeaderMap::new(), json!([report()]).to_string()).await;
        let mut reports = fixture.broker.history_reports().await.unwrap();
        assert_eq!(reports.len(), 1);
        let report = &mut reports[0];
        assert_eq!(report.report_id, "9223372036854775807");
        assert_eq!(report.status, HistoryReportStatus::Finished);
        let (from, to) = bounds();
        assert!(report.matches(from, to));
        assert!(!report.matches(from + chrono::TimeDelta::microseconds(1), to));
        assert!(!report.matches(from, to + chrono::TimeDelta::seconds(1)));
        report.data_included.include_transactions = false;
        assert!(!report.matches(from, to));
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[0].path, "/api/v0/equity/history/exports");
        assert!(requests[0].body.is_empty());
    }

    #[tokio::test]
    async fn status_flags_bounds_states_and_identifiers_fail_closed() {
        let mut missing_flag = report();
        missing_flag["dataIncluded"].as_object_mut().unwrap().remove("includeInterest");
        let mut bad_flag = report();
        bad_flag["dataIncluded"]["includeInterest"] = json!("true");
        let mut bad_state = report();
        bad_state["status"] = json!("finished");
        let mut missing_bounds = report();
        missing_bounds.as_object_mut().unwrap().remove("timeFrom");
        let mut invalid_bounds = report();
        invalid_bounds["timeTo"] = json!("not-a-timestamp");
        let mut float_id = report();
        float_id["reportId"] = json!(12.5);
        let mut string_id = report();
        string_id["reportId"] = json!("9007199254740993");
        let mut oversized_id = report();
        oversized_id["reportId"] = json!(9223372036854775808_u64);
        for invalid in [missing_flag, bad_flag, bad_state, missing_bounds, invalid_bounds, float_id, string_id, oversized_id] {
            let fixture = fixture(StatusCode::OK, HeaderMap::new(), json!([invalid]).to_string()).await;
            let error = fixture.broker.history_reports().await.err().expect("invalid schema rejected");
            let typed = error.downcast_ref::<HistoryHttpError>().unwrap();
            assert_eq!(typed.code, "invalid_schema");
            assert_eq!(typed.status, 200);
            assert!(!format!("{error:#}").contains("do-not-persist"));
            assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn report_creation_rejects_missing_or_non_int64_identifiers() {
        for body in ["{}", "{\"reportId\":1.5}", "{\"reportId\":\"123\"}", "{\"reportId\":9223372036854775808}", "not-json"] {
            let fixture = fixture(StatusCode::OK, HeaderMap::new(), body.into()).await;
            let (from, to) = bounds();
            let error = fixture.broker.request_history_report(from, to).await.unwrap_err();
            assert_eq!(error.downcast_ref::<HistoryHttpError>().unwrap().code, "invalid_schema");
            assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn documented_report_states_are_exact() {
        for (name, expected) in [
            ("Queued", HistoryReportStatus::Queued),
            ("Processing", HistoryReportStatus::Processing),
            ("Running", HistoryReportStatus::Running),
            ("Canceled", HistoryReportStatus::Canceled),
            ("Failed", HistoryReportStatus::Failed),
            ("Finished", HistoryReportStatus::Finished),
        ] {
            let mut value = report();
            value["status"] = json!(name);
            let parsed: HistoryReport = serde_json::from_value(value).ok().unwrap();
            assert_eq!(parsed.status, expected);
        }
        for name in ["Cancelled", "FINISHED", "Unknown"] {
            let mut value = report();
            value["status"] = json!(name);
            assert!(serde_json::from_value::<HistoryReport>(value).is_err());
        }
    }

    #[tokio::test]
    async fn definite_history_errors_are_safe_actionable_and_never_retried() {
        for (status, code) in [
            (StatusCode::BAD_REQUEST, "bad_filtering_arguments"),
            (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            (StatusCode::FORBIDDEN, "export_unavailable"),
            (StatusCode::REQUEST_TIMEOUT, "request_timeout"),
            (StatusCode::SERVICE_UNAVAILABLE, "broker_unavailable"),
        ] {
            for create in [false, true] {
                let fixture = fixture(status, HeaderMap::new(), "history-fixture-key history-fixture-secret https://secret.example/?signature=do-not-persist".into()).await;
                let error = if create {
                    let (from, to) = bounds();
                    fixture.broker.request_history_report(from, to).await.err().unwrap()
                } else {
                    fixture.broker.history_reports().await.err().unwrap()
                };
                let typed = error.downcast_ref::<HistoryHttpError>().unwrap();
                assert_eq!(typed.status, status.as_u16());
                assert_eq!(typed.code, code);
                if status == StatusCode::FORBIDDEN {
                    assert!(!typed.message.contains("history/export access"));
                    assert!(typed.allows_history_list_fallback());
                }
                let diagnostic = format!("{error:#}");
                for sensitive in ["history-fixture-key", "history-fixture-secret", "secret.example", "do-not-persist"] {
                    assert!(!diagnostic.contains(sensitive));
                }
                assert_eq!(fixture.requests.lock().unwrap().len(), 1);
            }
        }
    }

    #[test]
    fn forbidden_bodies_name_a_missing_scope_or_demo_export_limit() {
        let demo = HistoryHttpError::forbidden(br#"{"type":"/api-errors/not-available-in-demo-account"}"#);
        assert_eq!(demo.code, "export_unavailable_demo");
        assert!(demo.allows_history_list_fallback());
        assert!(!demo.message.contains("Enable history"));
        let scope = HistoryHttpError::forbidden(br#"Scope(history:orders) missing for API key"#);
        assert!(scope.denies_history_lists());
        assert!(scope.message.contains("history:orders"));
        assert!(!scope.message.contains("history-fixture-secret"));
        let edge = HistoryHttpError::forbidden(b"<html>Access Denied cloudflare</html>");
        assert_eq!(edge.code, "broker_edge_blocked");
        assert!(edge.allows_history_list_fallback());
    }

    #[tokio::test]
    async fn rate_limits_use_common_deadlines_without_resending_exports() {
        for create in [false, true] {
            for explicit_delay in [false, true] {
                let mut headers = HeaderMap::new();
                if explicit_delay {
                    headers.insert("retry-after", "120".parse().unwrap());
                }
                let fixture = fixture(StatusCode::TOO_MANY_REQUESTS, headers, "do-not-persist".into()).await;
                let error = if create {
                    let (from, to) = bounds();
                    fixture.broker.request_history_report(from, to).await.err().unwrap()
                } else {
                    fixture.broker.history_reports().await.err().unwrap()
                };
                let limited = error.downcast_ref::<BrokerRateLimited>().unwrap();
                let expected = if explicit_delay { 120 } else if create { 31 } else { 61 };
                assert!(limited.retry_after_seconds >= expected);
                assert_eq!(fixture.requests.lock().unwrap().len(), 1);
            }
        }
    }
}
