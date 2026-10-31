use axum::{extract::{rejection::JsonRejection, Query}, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use tokio::fs;
use utoipa::IntoParams;

use crate::{
    database::queries::{
        composite::{events_exist, EventFilter},
        performance::get_performance_signals,
    },
    services::{
        calculations::{self, CalculationSchedulerSettings, CalculationStatus},
        performance::compute_performance,
        shared::{constants::OUT_DIR, env::is_running_in_docker},
    },
};

use super::{internal_error, json_response, security_accounting_error};
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
        (status = 200, description = "Live economic EUR security P&L: trading realized + unrealized + recorded gross EUR dividends, including signed corrections through the valuation cutoff. Dividends use canonical ISINs and the requested broker; closed and dividend-only instruments are included. Fees are included; withholding taxes, interest, projected income and deemed distributions are excluded. Return uses cumulative gross purchase spend, or 0% without purchases. Optional alpha compares actual P&L including dividends with a nominal SP500 price-index proxy, not a dividend-reinvestment or currency-adjusted benchmark.", body = crate::services::performance::PortfolioPerformance),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Incomplete acquisition history; import or reconcile the reported acquisitions or transfers", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn live_performance(
    Query(query): Query<PerformanceQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let performance = compute_performance(query.isin.as_deref(), query.broker.as_deref())
        .await
        .map_err(|error| security_accounting_error(error, "PerformanceComputationError"))?;
    json_response(&performance).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/performance_overview",
    tag = "performance",
    responses(
        (status = 200, description = "Precomputed economic EUR security performance from output/performance.json, including recorded gross dividends; realized remains trading-only. Regenerate with `doughbox performance` after changing income records.", body = crate::services::performance::PortfolioPerformance),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No saved performance; press Refresh performance or run `doughbox performance`", body = ErrorResponse),
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
            let events_check_result = events_exist(EventFilter::All).await.map_err(|e| {
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

#[utoipa::path(
    get, path = "/api/performance/status", tag = "performance",
    responses(
        (status = 200, description = "Persistent manual and scheduled refresh status and opt-in schedule; reading status does not calculate performance", body = CalculationStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Could not read refresh status", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn get_performance_status() -> Result<impl IntoResponse, ErrorResponse> {
    let status = calculations::status("performance").await
        .map_err(|_| internal_error("PerformanceRefreshStatusError", "Could not read performance refresh status"))?;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/performance/refresh", tag = "performance",
    responses(
        (status = 202, description = "Started asynchronous performance export and portfolio snapshot, or retained the existing active refresh; does not change the schedule", body = CalculationStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Could not start refresh", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn refresh_performance() -> Result<impl IntoResponse, ErrorResponse> {
    let status = calculations::refresh("performance", None, None).await
        .map_err(|_| internal_error("PerformanceRefreshError", "Could not start performance refresh"))?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[utoipa::path(
    put, path = "/api/performance/scheduler", tag = "performance",
    request_body = CalculationSchedulerSettings,
    responses(
        (status = 200, description = "Persisted opt-in performance schedule (disabled by default, monthly cadence). Enabling or changing an enabled cadence makes the full performance export and guarded portfolio snapshot due on the next API scheduler tick, within one minute. Identical saves preserve the deadline. Pausing clears the deadline without cancelling active work.", body = CalculationStatus),
        (status = 400, description = "Malformed JSON", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 415, description = "Request requires a JSON content type", body = ErrorResponse),
        (status = 422, description = "Invalid enabled flag or frequency; supported frequencies are daily, weekly, monthly", body = ErrorResponse),
        (status = 500, description = "Could not save performance schedule", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn save_performance_scheduler(
    payload: Result<Json<CalculationSchedulerSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(settings) = payload.map_err(|rejection| {
        ErrorResponse::new(
            rejection.status(),
            "InvalidRequest",
            &rejection.body_text(),
            None,
        )
    })?;
    let status = calculations::save_schedule("performance", settings)
        .await
        .map_err(|_| internal_error("PerformanceScheduleError", "Could not save performance schedule"))?;
    Ok(Json(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, extract::{FromRequest, Request}};

    #[tokio::test]
    async fn invalid_performance_schedules_are_rejected_before_database_access() {
        for (body, content_type, expected) in [
            (r#"{"enabled":true,"frequency":"hourly"}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":"true","frequency":"daily"}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":true}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"frequency":"monthly"}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":true,"frequency":"daily""#, Some("application/json"), StatusCode::BAD_REQUEST),
            (r#"{"enabled":true,"frequency":"daily"}"#, None, StatusCode::UNSUPPORTED_MEDIA_TYPE),
            (r#"{"enabled":false,"frequency":"weekly"}"#, Some("text/plain"), StatusCode::UNSUPPORTED_MEDIA_TYPE),
        ] {
            let mut request = Request::builder();
            if let Some(content_type) = content_type {
                request = request.header("content-type", content_type);
            }
            let request = request.body(Body::from(body)).unwrap();
            let payload = Json::<CalculationSchedulerSettings>::from_request(request, &()).await;
            let result = save_performance_scheduler(payload).await;
            assert_eq!(result.err().unwrap().into_response().status(), expected);
        }
    }
}
