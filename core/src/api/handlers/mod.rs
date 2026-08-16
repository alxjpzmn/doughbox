pub mod auth;
pub mod market;
pub mod performance;
pub mod portfolio;
pub mod taxation;
pub mod timeline;
pub mod trades;

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use chrono::{DateTime, NaiveDate, Utc};
use tower_sessions::Session;

use crate::services::shared::{constants::SESSION_TOKEN_KEY, env::get_env_variable};

use super::errors::ErrorResponse;

pub fn json_response<T: serde::Serialize>(
    data: &T,
) -> Result<(StatusCode, HeaderMap, String), StatusCode> {
    let data = serde_json::to_string(data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    Ok((StatusCode::OK, headers, data))
}

pub fn parse_ymd_start(value: &str) -> Result<DateTime<Utc>, ErrorResponse> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|ndt| DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
        .ok_or_else(|| {
            ErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "InvalidDate",
                &format!("Invalid date '{value}', expected YYYY-MM-DD"),
                None,
            )
        })
}

pub fn parse_ymd_end(value: &str) -> Result<DateTime<Utc>, ErrorResponse> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(23, 59, 59))
        .map(|ndt| DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
        .ok_or_else(|| {
            ErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "InvalidDate",
                &format!("Invalid date '{value}', expected YYYY-MM-DD"),
                None,
            )
        })
}

pub fn parse_optional_ymd_start(
    value: Option<&str>,
) -> Result<Option<DateTime<Utc>>, ErrorResponse> {
    value.map(parse_ymd_start).transpose()
}

pub fn parse_optional_ymd_end(value: Option<&str>) -> Result<Option<DateTime<Utc>>, ErrorResponse> {
    value.map(parse_ymd_end).transpose()
}

pub fn internal_error(error: &str, message: impl ToString) -> ErrorResponse {
    ErrorResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        error,
        &message.to_string(),
        None,
    )
}

pub async fn check_auth(
    session: Session,
    request: Request,
    next: Next,
) -> anyhow::Result<Response, StatusCode> {
    if session
        .get::<String>(SESSION_TOKEN_KEY)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some()
    {
        return Ok(next.run(request).await);
    }

    if let Some(token) = get_env_variable("API_TOKEN") {
        if let Some(auth_header) = request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
        {
            if auth_header.strip_prefix("Bearer ") == Some(&token) {
                return Ok(next.run(request).await);
            }
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}
