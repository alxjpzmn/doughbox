use axum::{extract::Query, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use tokio::fs;
use utoipa::IntoParams;

use crate::{
    database::queries::{
        composite::{events_exist, EventFilter},
        performance::get_performance_signals,
    },
    services::{
        performance::compute_performance,
        shared::{constants::OUT_DIR, env::is_running_in_docker},
    },
};

use super::{internal_error, json_response};
use crate::api::errors::{ErrorDetails, ErrorResponse};

#[derive(Debug, Deserialize, IntoParams)]
pub struct PerformanceQuery {
    pub isin: Option<String>,
    pub broker: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/performance",
    tag = "performance",
    params(PerformanceQuery),
    responses(
        (status = 200, description = "Live realized/unrealized performance", body = crate::services::performance::PortfolioPerformance),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn live_performance(
    Query(query): Query<PerformanceQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let performance = compute_performance(query.isin.as_deref(), query.broker.as_deref())
        .await
        .map_err(|e| internal_error("PerformanceComputationError", e))?;
    json_response(&performance).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/performance_overview",
    tag = "performance",
    responses(
        (status = 200, description = "Precomputed performance from output/performance.json", body = crate::services::performance::PortfolioPerformance),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "File not found; run `doughbox performance` first", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn performance_overview() -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    let path = format!("{}/performance.json", OUT_DIR);
    match fs::read_to_string(&path).await {
        Ok(data) => {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse().unwrap());
            Ok((StatusCode::OK, headers, data))
        }
        Err(err) => {
            let events_check_result = events_exist(EventFilter::TradesOnly).await.map_err(|e| {
                internal_error(
                    "EventsExistError",
                    format!("Error while checking if events exist: {e}"),
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
                    &format!("The file '{path}' could not be found."),
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

#[utoipa::path(
    get,
    path = "/api/past_performance",
    tag = "performance",
    responses(
        (status = 200, description = "Historical portfolio value snapshots", body = [crate::database::models::performance::PerformanceSignal]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(("api_token" = []))
)]
pub async fn past_performance() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let performance = get_performance_signals()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&performance)
}
