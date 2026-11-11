use std::{collections::BTreeMap, fmt, str::FromStr};

use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};
use serde_json::{Map, Number, Value};

use super::{rejection_detail, BrokerClient};
use crate::services::trading::pies::{
    PieCreateSettings, PieDetail, PieDividendDetails, PieInstrument, PieIssue, PieList,
    PieSettings, PieSummary, PieValueResult,
};

/// Only RateLimited and Rejected prove the mutation was refused. Unknown must
/// never be automatically replayed. A GET 404 is represented by Ok(None) only.
#[derive(Debug)]
pub enum PieBrokerError {
    Rejected(String),
    Unknown(String),
    /// A selected source could not be decoded/identified; never try another ID.
    MalformedResponse(String),
    RateLimited {
        retry_after_seconds: u64,
        message: String,
    },
}

impl fmt::Display for PieBrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(message)
            | Self::Unknown(message)
            | Self::MalformedResponse(message)
            | Self::RateLimited { message, .. } => f.write_str(message),
        }
    }
}
impl std::error::Error for PieBrokerError {}

type Result<T, E = PieBrokerError> = std::result::Result<T, E>;

fn malformed() -> PieBrokerError {
    PieBrokerError::Unknown(
        "Trading 212 returned malformed Pie data; no mutation should be retried automatically"
            .into(),
    )
}

pub(super) fn positive_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|c| c.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|id| id > 0)
}

fn id(value: &Value) -> Result<String> {
    let raw = match value {
        Value::String(raw) => raw.clone(),
        Value::Number(raw) => raw.to_string(),
        _ => return Err(malformed()),
    };
    if !positive_id(&raw) {
        return Err(malformed());
    }
    // IDs are canonical decimal strings, never JavaScript numbers.
    Ok(raw.parse::<i64>().map_err(|_| malformed())?.to_string())
}

fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(malformed)
}
fn text(value: Option<&Value>) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => Ok(Some(raw.clone())),
        _ => Err(malformed()),
    }
}
fn number(value: Option<&Value>) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(raw)) => Ok(Some(raw.to_string())),
        Some(Value::String(raw)) if Number::from_str(raw).is_ok() => Ok(Some(raw.clone())),
        _ => Err(malformed()),
    }
}
fn nested<T>(value: Option<&Value>, decode: impl FnOnce(&Value) -> Result<T>) -> Result<Option<T>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => decode(value).map(Some),
    }
}
fn value_result(value: &Value) -> Result<PieValueResult> {
    let v = object(value)?;
    Ok(PieValueResult {
        price_avg_invested_value: number(v.get("priceAvgInvestedValue"))?,
        price_avg_result: number(v.get("priceAvgResult"))?,
        price_avg_result_coef: number(v.get("priceAvgResultCoef"))?,
        price_avg_value: number(v.get("priceAvgValue"))?,
    })
}
fn dividend(value: &Value) -> Result<PieDividendDetails> {
    let v = object(value)?;
    Ok(PieDividendDetails {
        gained: number(v.get("gained"))?,
        in_cash: number(v.get("inCash"))?,
        reinvested: number(v.get("reinvested"))?,
    })
}
fn summary(value: &Value) -> Result<PieSummary> {
    let v = object(value)?;
    Ok(PieSummary {
        id: id(v.get("id").ok_or_else(malformed)?)?,
        cash: number(v.get("cash"))?,
        dividend_details: nested(v.get("dividendDetails"), dividend)?,
        progress: number(v.get("progress"))?,
        result: nested(v.get("result"), value_result)?,
        status: text(v.get("status"))?,
    })
}
fn settings(value: &Value) -> Result<PieSettings> {
    let v = object(value)?;
    let instrument_shares = nested(v.get("instrumentShares"), |value| {
        object(value)?
            .iter()
            .map(|(ticker, weight)| Ok((ticker.clone(), number(Some(weight))?)))
            .collect::<Result<BTreeMap<_, _>>>()
    })?;
    Ok(PieSettings {
        name: text(v.get("name"))?,
        icon: text(v.get("icon"))?,
        dividend_cash_action: text(v.get("dividendCashAction"))?,
        goal: number(v.get("goal"))?,
        end_date: text(v.get("endDate"))?,
        instrument_shares,
        instrument_shares_source: None,
    })
}
fn instrument(value: &Value) -> Result<PieInstrument> {
    let v = object(value)?;
    let issues = nested(v.get("issues"), |value| {
        value
            .as_array()
            .ok_or_else(malformed)?
            .iter()
            .map(|issue| {
                let issue = object(issue)?;
                Ok(PieIssue {
                    name: text(issue.get("name"))?,
                    severity: text(issue.get("severity"))?,
                })
            })
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(PieInstrument {
        ticker: text(v.get("ticker"))?,
        owned_quantity: number(v.get("ownedQuantity"))?,
        current_share: number(v.get("currentShare"))?,
        expected_share: number(v.get("expectedShare"))?,
        issues,
        result: nested(v.get("result"), value_result)?,
    })
}
fn detail(value: &Value, expected_id: Option<&str>) -> Result<PieDetail> {
    let v = object(value)?;
    // The documented acknowledgement identifies a Pie through settings.id.
    let raw_settings = v.get("settings").ok_or_else(malformed)?;
    let setting_object = object(raw_settings)?;
    let pie_id = id(setting_object.get("id").ok_or_else(malformed)?)?;
    if let Some(route_id) = expected_id {
        if route_id.parse::<i64>().ok() != pie_id.parse::<i64>().ok() {
            return Err(malformed());
        }
    }
    if let Some(outer_id) = v.get("id") {
        if id(outer_id)? != pie_id {
            return Err(malformed());
        }
    }
    let instruments = nested(v.get("instruments"), |value| {
        value
            .as_array()
            .ok_or_else(malformed)?
            .iter()
            .map(instrument)
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(PieDetail {
        fetched_at: Utc::now(),
        id: pie_id,
        settings: Some(settings(raw_settings)?),
        instruments,
    })
}

fn numeric(raw: &str) -> Result<Value> {
    Number::from_str(raw)
        .map(Value::Number)
        .map_err(|_| PieBrokerError::Rejected("Pie setting could not be encoded exactly".into()))
}
fn payload(settings: &PieCreateSettings) -> Result<Value> {
    let mut body = Map::new();
    body.insert("name".into(), Value::String(settings.name.clone()));
    body.insert(
        "dividendCashAction".into(),
        serde_json::to_value(&settings.dividend_cash_action).map_err(|_| malformed())?,
    );
    let shares = settings
        .instrument_shares
        .iter()
        .map(|(ticker, weight)| Ok((ticker.clone(), numeric(weight)?)))
        .collect::<Result<Map<_, _>>>()?;
    body.insert("instrumentShares".into(), Value::Object(shares));
    if let Some(icon) = &settings.icon {
        body.insert("icon".into(), Value::String(icon.clone()));
    }
    if let Some(goal) = &settings.goal {
        body.insert("goal".into(), numeric(goal)?);
    }
    if let Some(end_date) = &settings.end_date {
        body.insert("endDate".into(), Value::String(end_date.clone()));
    }
    Ok(Value::Object(body))
}

fn retry_after(headers: &reqwest::header::HeaderMap, fallback: u64) -> u64 {
    let Some(raw) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
    else {
        return fallback;
    };
    if let Ok(seconds) = raw.parse::<u64>() {
        return seconds.max(fallback);
    }
    // HTTP-date (IMF-fixdate and obsolete RFC 2822 variants), rounded up.
    let date = DateTime::parse_from_rfc2822(raw)
        .map(|date| date.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%A, %d-%b-%y %H:%M:%S GMT")
                .ok()
                .map(|date| date.and_utc())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%a %b %e %H:%M:%S %Y")
                .ok()
                .map(|date| date.and_utc())
        });
    date.map(|date| {
        let millis = date.signed_duration_since(Utc::now()).num_milliseconds();
        if millis <= 0 {
            fallback
        } else {
            ((millis as u64).saturating_add(999) / 1000).max(fallback)
        }
    })
    .unwrap_or(fallback)
}

impl BrokerClient {
    async fn pie_request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        list: bool,
        absent: bool,
    ) -> Result<Option<reqwest::Response>> {
        let mut request = self.request(method, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|_| PieBrokerError::Unknown(
            "Trading 212 Pie request could not be confirmed; do not retry mutations automatically".into()))?;
        let status = response.status();
        if status == StatusCode::OK {
            return Ok(Some(response));
        }
        if absent && status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let delay = retry_after(response.headers(), if list { 31 } else { 6 });
        if status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT {
            let diagnostic = rejection_detail(response, &self.credentials).await;
            let message = format!(
                "Trading 212 rejected the Pie request (HTTP {}): {diagnostic}",
                status.as_u16()
            );
            if status == StatusCode::TOO_MANY_REQUESTS {
                return Err(PieBrokerError::RateLimited {
                    retry_after_seconds: delay,
                    message,
                });
            }
            return Err(PieBrokerError::Rejected(message));
        }
        Err(PieBrokerError::Unknown(format!("Trading 212 Pie request outcome is uncertain (HTTP {}); do not retry mutations automatically", status.as_u16())))
    }

    pub async fn pies(&self) -> Result<PieList> {
        let response = self
            .pie_request(Method::GET, "/equity/pies", None, true, false)
            .await?
            .ok_or_else(malformed)?;
        let value: Value = response.json().await.map_err(|_| malformed())?;
        let items = value
            .as_array()
            .ok_or_else(malformed)?
            .iter()
            .map(summary)
            .collect::<Result<Vec<_>>>()?;
        Ok(PieList {
            fetched_at: Utc::now(),
            items,
        })
    }
    pub async fn pie(&self, pie_id: &str) -> Result<Option<PieDetail>> {
        check_id(pie_id)?;
        let Some(response) = self
            .pie_request(
                Method::GET,
                &format!("/equity/pies/{pie_id}"),
                None,
                false,
                true,
            )
            .await?
        else {
            return Ok(None);
        };
        let value = response
            .json()
            .await
            .map_err(|_| PieBrokerError::MalformedResponse(malformed().to_string()))?;
        detail(&value, Some(pie_id))
            .map(Some)
            .map_err(|error| PieBrokerError::MalformedResponse(error.to_string()))
    }
    pub async fn create_pie(&self, settings: &PieCreateSettings) -> Result<PieDetail> {
        let response = self
            .pie_request(
                Method::POST,
                "/equity/pies",
                Some(payload(settings)?),
                false,
                false,
            )
            .await?
            .ok_or_else(malformed)?;
        let value = response.json().await.map_err(|_| malformed())?;
        detail(&value, None)
    }
    pub async fn update_pie(&self, pie_id: &str, settings: &PieCreateSettings) -> Result<()> {
        check_id(pie_id)?;
        self.pie_request(
            Method::POST,
            &format!("/equity/pies/{pie_id}"),
            Some(payload(settings)?),
            false,
            false,
        )
        .await
        .map_err(|error| annotate_rename_rejection(pie_id, error))?;
        Ok(())
    }
    pub async fn duplicate_pie(
        &self,
        pie_id: &str,
        name: &str,
        icon: Option<&str>,
    ) -> Result<PieDetail> {
        check_id(pie_id)?;
        let mut body = Map::new();
        body.insert("name".into(), Value::String(name.to_owned()));
        if let Some(icon) = icon {
            body.insert("icon".into(), Value::String(icon.to_owned()));
        }
        let response = self
            .pie_request(
                Method::POST,
                &format!("/equity/pies/{pie_id}/duplicate"),
                Some(Value::Object(body)),
                false,
                false,
            )
            .await?
            .ok_or_else(malformed)?;
        let value = response.json().await.map_err(|_| malformed())?;
        let copy = detail(&value, None)?;
        if copy.id.parse::<i64>().ok() == pie_id.parse::<i64>().ok() {
            return Err(malformed());
        }
        Ok(copy)
    }
    pub async fn delete_pie(&self, pie_id: &str) -> Result<()> {
        check_id(pie_id)?;
        self.pie_request(
            Method::DELETE,
            &format!("/equity/pies/{pie_id}"),
            None,
            false,
            false,
        )
        .await?;
        Ok(())
    }
}
fn check_id(pie_id: &str) -> Result<()> {
    if positive_id(pie_id) {
        Ok(())
    } else {
        Err(PieBrokerError::Rejected(
            "Pie ID must be a positive int64 decimal string".into(),
        ))
    }
}
fn annotate_rename_rejection(pie_id: &str, error: PieBrokerError) -> PieBrokerError {
    match error {
        PieBrokerError::Rejected(message)
            if message.contains("rename the pie if you want to apply changes") =>
        {
            PieBrokerError::Rejected(format!(
                "{message} Set the next versioned name in a new proposal for Pie #{pie_id}: increment a trailing \" vN\", otherwise append \" v1\". This request will not be resent."
            ))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Credentials;
    use super::*;
    use crate::services::trading::pies::DividendCashAction;
    use serde_json::json;
    use std::{
        borrow::Cow,
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };
    #[test]
    fn rename_rejection_keeps_broker_detail_and_blocks_resend() {
        let annotated = annotate_rename_rejection(
            "941527",
            PieBrokerError::Rejected(
                "Trading 212 rejected the Pie request (HTTP 400): The request is invalid; Error updating pie with id 941527. Check the id and rename the pie if you want to apply changes.; /api-errors/invalid-request".into(),
            ),
        );
        let message = annotated.to_string();
        assert!(message.contains("Pie #941527"), "{message}");
        assert!(message.contains("append \" v1\""), "{message}");
        assert!(message.contains("will not be resent"), "{message}");
        assert!(message.contains("/api-errors/invalid-request"), "{message}");
        let other = annotate_rename_rejection(
            "941527",
            PieBrokerError::Rejected("instrumentShares must sum to 1".into()),
        );
        assert_eq!(other.to_string(), "instrumentShares must sum to 1");
        assert!(matches!(
            annotate_rename_rejection("941527", PieBrokerError::Unknown("timeout".into())),
            PieBrokerError::Unknown(_)
        ));
    }

    struct Reply {
        status: StatusCode,
        body: String,
        retry_after: Option<String>,
        delay: Duration,
    }
    impl Reply {
        fn new(status: StatusCode, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
                retry_after: None,
                delay: Duration::ZERO,
            }
        }
    }
    struct Fixture {
        broker: BrokerClient,
        calls: Arc<Mutex<Vec<(Method, String, Value, String)>>>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    async fn fixture(replies: Vec<Reply>) -> Fixture {
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let recorded = recorded.clone();
            let replies = replies.clone();
            async move {
                let method = request.method().clone();
                let path = request.uri().path().to_owned();
                let authorization = request
                    .headers()
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                let bytes = axum::body::to_bytes(request.into_body(), 65536)
                    .await
                    .unwrap();
                let body = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                recorded
                    .lock()
                    .unwrap()
                    .push((method, path, body, authorization));
                let reply = replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected second request");
                tokio::time::sleep(reply.delay).await;
                let mut response = axum::http::Response::builder().status(reply.status);
                if let Some(delay) = reply.retry_after {
                    response = response.header("retry-after", delay);
                }
                response.body(axum::body::Body::from(reply.body)).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = Cow::Owned(format!("http://{}/api/v0", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fixture {
            broker: BrokerClient {
                client: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap(),
                base_url,
                credentials: Credentials {
                    api_key: "fixture-key".into(),
                    api_secret: "fixture-secret".into(),
                },
            },
            calls,
            server,
        }
    }
    fn settings() -> PieCreateSettings {
        PieCreateSettings {
            name: "Approved exact name".into(),
            dividend_cash_action: DividendCashAction::REINVEST,
            instrument_shares: [
                ("AAPL_US_EQ".into(), "0.7000".into()),
                ("MSFT_US_EQ".into(), "0.3000".into()),
            ]
            .into(),
            icon: None,
            goal: Some("10000.0001".into()),
            end_date: Some("2030-01-01T00:00:00Z".into()),
        }
    }
    fn detail_body(id: i64) -> String {
        format!(
            r#"{{"settings":{{"id":{id},"name":"Approved exact name","dividendCashAction":"REINVEST","instrumentShares":{{"AAPL_US_EQ":0.7000,"MSFT_US_EQ":0.3000}}}},"instruments":[{{"ticker":"AAPL_US_EQ","ownedQuantity":0,"currentShare":0.6000,"expectedShare":0.7000}}]}}"#
        )
    }

    #[tokio::test]
    async fn native_routes_send_only_documented_exact_payloads() {
        let fixture = fixture(vec![
            Reply::new(
                StatusCode::OK,
                r#"[{"id":10,"cash":0.0000000000000000001,"progress":0,"status":"READY"}]"#,
            ),
            Reply::new(StatusCode::OK, detail_body(10)),
            Reply::new(StatusCode::OK, detail_body(11)),
            Reply::new(StatusCode::OK, "not required to be JSON"),
            Reply::new(StatusCode::OK, detail_body(12)),
            Reply::new(StatusCode::OK, "successful non-JSON deletion"),
        ])
        .await;
        let list = fixture.broker.pies().await.unwrap();
        assert_eq!(list.items[0].id, "10");
        assert_eq!(list.items[0].cash.as_deref(), Some("0.0000000000000000001"));
        let detail = fixture.broker.pie("10").await.unwrap().unwrap();
        assert_eq!(
            detail.instruments.as_ref().unwrap()[0]
                .owned_quantity
                .as_deref(),
            Some("0")
        );
        assert_eq!(
            detail.instruments.as_ref().unwrap()[0]
                .current_share
                .as_deref(),
            Some("0.6000")
        );
        assert_eq!(
            detail
                .settings
                .as_ref()
                .unwrap()
                .instrument_shares
                .as_ref()
                .unwrap()["AAPL_US_EQ"]
                .as_deref(),
            Some("0.7000")
        );
        assert_eq!(
            fixture.broker.create_pie(&settings()).await.unwrap().id,
            "11"
        );
        fixture.broker.update_pie("10", &settings()).await.unwrap();
        assert_eq!(
            fixture
                .broker
                .duplicate_pie("10", "Copy", Some("Requested icon"))
                .await
                .unwrap()
                .id,
            "12"
        );
        fixture.broker.delete_pie("20").await.unwrap();
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(calls.len(), 6);
        for (call, expected) in calls.iter().zip([
            (Method::GET, "/api/v0/equity/pies"),
            (Method::GET, "/api/v0/equity/pies/10"),
            (Method::POST, "/api/v0/equity/pies"),
            (Method::POST, "/api/v0/equity/pies/10"),
            (Method::POST, "/api/v0/equity/pies/10/duplicate"),
            (Method::DELETE, "/api/v0/equity/pies/20"),
        ]) {
            assert_eq!(call.0, expected.0);
            assert_eq!(call.1, expected.1);
            assert!(call.3.starts_with("Basic "));
        }
        assert_eq!(calls[2].2, payload(&settings()).unwrap());
        assert_eq!(calls[3].2, payload(&settings()).unwrap());
        assert!(calls[2].2["instrumentShares"]["AAPL_US_EQ"].is_number());
        assert_eq!(
            calls[2].2["instrumentShares"]["AAPL_US_EQ"].to_string(),
            "0.7000"
        );
        assert!(!calls[2].2.as_object().unwrap().contains_key("icon"));
        assert_eq!(calls[4].2, json!({"name":"Copy","icon":"Requested icon"}));
        assert_eq!(calls[5].2, Value::Null);
    }

    #[tokio::test]
    async fn only_authoritative_detail_404_is_absence() {
        let fixture = fixture(vec![
            Reply::new(StatusCode::NOT_FOUND, "absent"),
            Reply::new(StatusCode::FORBIDDEN, r#"{"message":"pies:read required"}"#),
            Reply::new(StatusCode::INTERNAL_SERVER_ERROR, "failure"),
        ])
        .await;
        assert!(fixture.broker.pie("10").await.unwrap().is_none());
        assert!(matches!(
            fixture.broker.pie("10").await,
            Err(PieBrokerError::Rejected(_))
        ));
        assert!(matches!(
            fixture.broker.pie("10").await,
            Err(PieBrokerError::Unknown(_))
        ));
        assert_eq!(fixture.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn missing_and_zero_quantities_remain_distinct_and_ids_are_strict() {
        let missing = detail(
            &json!({"settings":{"id":10},"instruments":[{"ticker":"AAPL_US_EQ"}]}),
            Some("10"),
        )
        .unwrap();
        assert!(missing.instruments.unwrap()[0].owned_quantity.is_none());
        assert!(detail(&json!({"settings":{"id":10}}), Some("10"))
            .unwrap()
            .instruments
            .is_none());
        assert!(
            detail(&json!({"settings":{"id":10},"instruments":[]}), Some("10"))
                .unwrap()
                .instruments
                .unwrap()
                .is_empty()
        );
        for value in [
            json!({"settings":{"id":0}}),
            json!({"settings":{"id":-1}}),
            json!({"settings":{"id":"9223372036854775808"}}),
            json!({"settings":{}}),
            json!({"id":10}),
            json!({"id":11,"settings":{"id":10}}),
        ] {
            assert!(detail(&value, None).is_err());
        }
        assert!(detail(&json!({"settings":{"id":11}}), Some("10")).is_err());
        assert_eq!(
            detail(&json!({"settings":{"id":"9223372036854775807"}}), None)
                .unwrap()
                .id,
            "9223372036854775807"
        );
    }

    #[tokio::test]
    async fn definite_rejections_are_redacted_and_never_retried_even_for_429() {
        let mut limited = Reply::new(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"message":"fixture-key fixture-secret refused"}"#,
        );
        limited.retry_after = Some("120".into());
        let fixture = fixture(vec![
            Reply::new(
                StatusCode::FORBIDDEN,
                r#"{"message":"pies:write permission denied"}"#,
            ),
            limited,
        ])
        .await;
        let error = fixture.broker.create_pie(&settings()).await.unwrap_err();
        assert!(matches!(error, PieBrokerError::Rejected(_)));
        assert!(error.to_string().contains("pies:write"));
        let error = fixture.broker.create_pie(&settings()).await.unwrap_err();
        assert!(matches!(
            error,
            PieBrokerError::RateLimited {
                retry_after_seconds: 120,
                ..
            }
        ));
        assert!(!error.to_string().contains("fixture-key"));
        assert!(!error.to_string().contains("fixture-secret"));
        assert_eq!(fixture.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn uncertain_acknowledgements_never_trigger_a_second_mutation() {
        for reply in [
            Reply::new(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"),
            Reply::new(StatusCode::REQUEST_TIMEOUT, "timeout"),
            Reply::new(StatusCode::OK, "not JSON"),
            Reply::new(
                StatusCode::OK,
                r#"{"id":11,"settings":{"name":"No settings ID"}}"#,
            ),
            Reply::new(StatusCode::CREATED, detail_body(11)),
        ] {
            let fixture = fixture(vec![reply]).await;
            assert!(matches!(
                fixture.broker.create_pie(&settings()).await,
                Err(PieBrokerError::Unknown(_))
            ));
            assert_eq!(fixture.calls.lock().unwrap().len(), 1);
        }
        let fixture = fixture(vec![Reply::new(StatusCode::OK, detail_body(10))]).await;
        assert!(matches!(
            fixture.broker.duplicate_pie("10", "Copy", None).await,
            Err(PieBrokerError::Unknown(_))
        ));
        assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn transport_timeout_does_not_replay_mutation() {
        let mut reply = Reply::new(StatusCode::OK, detail_body(11));
        reply.delay = Duration::from_millis(300);
        let mut fixture = fixture(vec![reply]).await;
        fixture.broker.client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_millis(75))
            .build()
            .unwrap();
        assert!(matches!(
            fixture.broker.create_pie(&settings()).await,
            Err(PieBrokerError::Unknown(_))
        ));
        assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn retry_after_never_shortens_longer_delays_and_invalid_values_use_bucket_interval() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers, 31), 31);
        headers.insert("retry-after", "invalid".parse().unwrap());
        assert_eq!(retry_after(&headers, 6), 6);
        headers.insert("retry-after", "120".parse().unwrap());
        assert_eq!(retry_after(&headers, 6), 120);
        headers.insert("retry-after", "1".parse().unwrap());
        assert_eq!(retry_after(&headers, 6), 6);
        let future = (Utc::now() + chrono::Duration::seconds(180)).to_rfc2822();
        headers.insert("retry-after", future.parse().unwrap());
        assert!(retry_after(&headers, 6) >= 179);
    }
}
