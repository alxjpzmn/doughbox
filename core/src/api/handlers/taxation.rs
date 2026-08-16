use axum::{extract::Query, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use tokio::fs;
use utoipa::IntoParams;

use crate::{
    database::queries::composite::{events_exist, EventFilter},
    services::{
        shared::{constants::OUT_DIR, env::is_running_in_docker},
        taxation::{
            get_capital_gains_tax_report, get_detailed_capital_gains_tax_report,
            get_transaction_tax_impacts,
        },
    },
};

use super::{internal_error, parse_optional_ymd_end, parse_optional_ymd_start};
use crate::api::errors::{ErrorDetails, ErrorResponse};

#[derive(Debug, Deserialize, IntoParams)]
pub struct TaxationQuery {
    pub from_date: Option<String>,
    pub until_date: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/taxation",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 200, description = "Tax report (live if dates given, else output/taxation.json)", body = crate::services::taxation::TaxationReport),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "File not found; run `doughbox taxation` first", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn taxation(
    Query(query): Query<TaxationQuery>,
) -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    if query.from_date.is_some() || query.until_date.is_some() {
        let from_date = parse_optional_ymd_start(query.from_date.as_deref())?;
        let until_date = parse_optional_ymd_end(query.until_date.as_deref())?;

        let report = get_capital_gains_tax_report(from_date, until_date)
            .await
            .map_err(|e| {
                log::error!("Taxation computation failed: {e}");
                internal_error(
                    "TaxationComputationError",
                    format!("Failed to compute taxation report: {e}"),
                )
            })?;

        let data = serde_json::to_string(&report).map_err(|e| {
            log::error!("Taxation report serialization failed: {e}");
            internal_error(
                "SerializationError",
                format!("Failed to serialize taxation report: {e}"),
            )
        })?;
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        return Ok((StatusCode::OK, headers, data));
    }

    let path = format!("{}/taxation.json", OUT_DIR);
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
    path = "/api/taxation/detailed",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 200, description = "Detailed tax report"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn taxation_detailed(
    Query(query): Query<TaxationQuery>,
) -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    let from_date = parse_optional_ymd_start(query.from_date.as_deref())?;
    let until_date = parse_optional_ymd_end(query.until_date.as_deref())?;

    let report = get_detailed_capital_gains_tax_report(from_date, until_date)
        .await
        .map_err(|e| {
            log::error!("Detailed taxation computation failed: {e}");
            internal_error(
                "TaxationComputationError",
                format!("Failed to compute detailed taxation report: {e}"),
            )
        })?;

    let data = serde_json::to_string(&report).map_err(|e| {
        log::error!("Detailed taxation report serialization failed: {e}");
        internal_error(
            "SerializationError",
            format!("Failed to serialize detailed taxation report: {e}"),
        )
    })?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
}

#[utoipa::path(
    get,
    path = "/api/taxation/transactions",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 200, description = "Per-transaction tax impacts", body = [crate::services::taxation::TransactionTaxImpact]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn taxation_transactions(
    Query(query): Query<TaxationQuery>,
) -> anyhow::Result<impl IntoResponse, ErrorResponse> {
    let from_date = parse_optional_ymd_start(query.from_date.as_deref())?;
    let until_date = parse_optional_ymd_end(query.until_date.as_deref())?;

    let impacts = get_transaction_tax_impacts(from_date, until_date)
        .await
        .map_err(|e| {
            log::error!("Transaction tax impacts computation failed: {e}");
            internal_error(
                "TaxationComputationError",
                format!("Failed to compute transaction tax impacts: {e}"),
            )
        })?;

    let data = serde_json::to_string(&impacts).map_err(|e| {
        log::error!("Transaction tax impacts serialization failed: {e}");
        internal_error(
            "SerializationError",
            format!("Failed to serialize transaction tax impacts: {e}"),
        )
    })?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
}
