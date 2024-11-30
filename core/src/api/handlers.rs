use crate::{
    services::portfolio::get_portfolio_overview,
    util::{
        constants::{OUT_DIR, SESSION_TOKEN_KEY},
        db_helpers::{get_dividends, get_performance_signals, get_positions},
        general_helpers::{get_env_variable, parse_timestamp},
        taxation_helpers::get_events,
    },
};
use axum::{
    extract::{Json, Query, Request},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;

use tokio::fs;
use tower_sessions::Session;

fn json_response<T: serde::Serialize>(
    data: &T,
) -> Result<(StatusCode, HeaderMap, String), StatusCode> {
    let data = serde_json::to_string(data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    Ok((StatusCode::OK, headers, data))
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

#[derive(Deserialize)]
pub struct LoginRequestData {
    pub password: String,
}

pub async fn issue_session_cookie(session: Session) -> anyhow::Result<(), StatusCode> {
    session
        .insert(SESSION_TOKEN_KEY, "user")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn login(
    session: Session,
    Json(payload): Json<LoginRequestData>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let password = get_env_variable("PASSWORD");
    match password {
        Some(password) if payload.password == password => {
            issue_session_cookie(session).await?;
            Ok(StatusCode::OK)
        }
        None => {
            issue_session_cookie(session).await?;
            Ok(StatusCode::OK)
        }
        _ => Ok(StatusCode::UNAUTHORIZED),
    }
}

pub async fn logout(session: Session) -> anyhow::Result<impl IntoResponse, StatusCode> {
    session
        .delete()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

pub async fn portfolio() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let portfolio_overview = get_portfolio_overview()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&portfolio_overview)
}

pub async fn performance() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let path = format!("{}/pl.json", OUT_DIR);
    let data = fs::read_to_string(path).await.expect("Unable to read file");
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
}

pub async fn past_performance() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let performance = get_performance_signals()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&performance)
}

#[derive(Debug, Deserialize)]
pub struct TimelineQuery {
    pub start_date: String,
}

pub async fn timeline(
    query: Query<TimelineQuery>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let year_start_timestamp = DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDate::parse_from_str(&query.start_date, "%Y-%m-%d")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        Utc,
    );

    let end_date = Utc::now();

    let timeline = get_events(year_start_timestamp, end_date)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&timeline)
}

pub async fn dividends() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let dividends = get_dividends()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&dividends)
}

pub async fn taxation() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let path = format!("{}/taxation.json", OUT_DIR);
    let data = fs::read_to_string(path).await.expect("Unable to read file");
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
}

#[derive(Debug, Deserialize)]
pub struct PositionsQuery {
    pub date: Option<String>,
}

pub async fn positions(
    Query(query): Query<PositionsQuery>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let date = query.date.unwrap_or_else(|| {
        let now = Utc::now();
        format!("{}-{:02}-{:02}", now.year(), now.month(), now.day())
    });
    let timestamp = parse_timestamp(format!("{} 19:00:00", date).as_str())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let positions = get_positions(Some(timestamp), None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&positions)
}

pub async fn auth_state() -> impl IntoResponse {
    (StatusCode::OK, "authenticated")
}
