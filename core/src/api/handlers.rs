use crate::{
    database::queries::{
        composite::{events_exist, EventFilter},
        performance::get_performance_signals,
    },
    services::{
        events::get_events,
        parsers::parse_timestamp,
        portfolio::get_portfolio_overview,
        positions::get_positions_overview,
        shared::{
            constants::{OUT_DIR, SESSION_TOKEN_KEY},
            env::{get_env_variable, is_running_in_docker},
        },
        taxation::{get_capital_gains_tax_report, get_detailed_capital_gains_tax_report},
    },
};
use axum::{
    extract::{Json, Query, Request},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use log;
use serde::Deserialize;

use tokio::fs;
use tower_sessions::Session;

use super::errors::{ErrorDetails, ErrorResponse};

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

pub async fn portfolio() -> Result<impl IntoResponse, ErrorResponse> {
    match get_portfolio_overview().await {
        Ok(portfolio_overview) => {
            if portfolio_overview.positions.is_empty() {
                let events_check_result =
                    events_exist(EventFilter::TradesOnly).await.map_err(|e| {
                        ErrorResponse::new(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "EventsExistError",
                            &format!("Error while checking if events exist: {}", e),
                            None,
                        )
                    })?;
                let error_details = ErrorDetails {
                    in_docker: Some(is_running_in_docker()),
                    events_present: Some(events_check_result),
                };

                if !events_check_result {
                    return Err(ErrorResponse::new(
                        StatusCode::NOT_FOUND,
                        "EmptyPortfolioError",
                        "Empty portfolio without events",
                        Some(error_details),
                    ));
                }
            }
            Ok(json_response(&portfolio_overview))
        }
        Err(_err) => {
            let events_check_result = events_exist(EventFilter::TradesOnly).await.map_err(|e| {
                ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "EventsExistError",
                    &format!("Error while checking if events exist: {}", e),
                    None,
                )
            })?;
            let error_details = ErrorDetails {
                in_docker: Some(is_running_in_docker()),
                events_present: Some(events_check_result),
            };
            // Handle the error by returning an ErrorResponse
            Err(ErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PortfolioRetrievalError",
                "An error occurred while retrieving the portfolio overview.",
                Some(error_details),
            ))
        }
    }
}

pub async fn performance() -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    let path = format!("{}/performance.json", OUT_DIR);
    match fs::read_to_string(&path).await {
        Ok(data) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse().unwrap());
            Ok((StatusCode::OK, headers, data))
        }
        Err(err) => {
            let events_check_result = events_exist(EventFilter::TradesOnly).await.map_err(|e| {
                ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "EventsExistError",
                    &format!("Error while checking if events exist: {}", e),
                    None,
                )
            })?;

            let error_details = ErrorDetails {
                in_docker: Some(is_running_in_docker()),
                events_present: Some(events_check_result),
            };
            if err.kind() == std::io::ErrorKind::NotFound {
                Err(ErrorResponse::new(
                    StatusCode::NOT_FOUND,
                    "FileNotFound",
                    &format!("The file '{}' could not be found.", path),
                    Some(error_details),
                ))
            } else {
                Err(ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalServerError",
                    "An unexpected error occurred while reading the file.",
                    Some(error_details),
                ))
            }
        }
    }
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

    let mut timeline = get_events(year_start_timestamp, end_date)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    timeline.sort_by(|a, b| b.date.cmp(&a.date));

    json_response(&timeline)
}

#[derive(Debug, Deserialize)]
pub struct TaxationQuery {
    pub from_date: Option<String>,
    pub until_date: Option<String>,
}

pub async fn taxation(
    Query(query): Query<TaxationQuery>,
) -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    if query.from_date.is_some() || query.until_date.is_some() {
        let from_date = query.from_date.as_deref().map(|d| {
            DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
                Utc,
            )
        });
        let until_date = query.until_date.as_deref().map(|d| {
            DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .unwrap()
                    .and_hms_opt(23, 59, 59)
                    .unwrap(),
                Utc,
            )
        });

        let report = get_capital_gains_tax_report(from_date, until_date)
            .await
            .map_err(|e| {
                log::error!("Taxation computation failed: {}", e);
                ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "TaxationComputationError",
                    &format!("Failed to compute taxation report: {}", e),
                    None,
                )
            })?;

        let data = serde_json::to_string(&report).map_err(|e| {
            log::error!("Taxation report serialization failed: {}", e);
            ErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SerializationError",
                &format!("Failed to serialize taxation report: {}", e),
                None,
            )
        })?;
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        return Ok((StatusCode::OK, headers, data));
    }

    let path = format!("{}/taxation.json", OUT_DIR);
    match fs::read_to_string(&path).await {
        Ok(data) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse().unwrap());
            Ok((StatusCode::OK, headers, data))
        }
        Err(err) => {
            let events_check_result = events_exist(EventFilter::All).await.map_err(|e| {
                ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "EventsExistError",
                    &format!("Error while checking if events exist: {}", e),
                    None,
                )
            })?;

            let error_details = ErrorDetails {
                in_docker: Some(is_running_in_docker()),
                events_present: Some(events_check_result),
            };
            if err.kind() == std::io::ErrorKind::NotFound {
                Err(ErrorResponse::new(
                    StatusCode::NOT_FOUND,
                    "FileNotFound",
                    &format!("The file '{}' could not be found.", path),
                    Some(error_details),
                ))
            } else {
                Err(ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalServerError",
                    "An unexpected error occurred while reading the file.",
                    Some(error_details),
                ))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PositionsQuery {
    pub date: Option<String>,
}

pub async fn taxation_detailed(
    Query(query): Query<TaxationQuery>,
) -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    let from_date = query.from_date.as_deref().map(|d| {
        DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            Utc,
        )
    });
    let until_date = query.until_date.as_deref().map(|d| {
        DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap(),
            Utc,
        )
    });

    let report = get_detailed_capital_gains_tax_report(from_date, until_date)
        .await
        .map_err(|e| {
            log::error!("Detailed taxation computation failed: {}", e);
            ErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "TaxationComputationError",
                &format!("Failed to compute detailed taxation report: {}", e),
                None,
            )
        })?;

    let data = serde_json::to_string(&report).map_err(|e| {
        log::error!("Detailed taxation report serialization failed: {}", e);
        ErrorResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "SerializationError",
            &format!("Failed to serialize detailed taxation report: {}", e),
            None,
        )
    })?;
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
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
    let positions = get_positions_overview(Some(timestamp))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&positions)
}

pub async fn auth_state() -> impl IntoResponse {
    (StatusCode::OK, "authenticated")
}
