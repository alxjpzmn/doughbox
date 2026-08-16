use axum::{extract::Path, http::StatusCode, response::IntoResponse};

use crate::database::queries::{
    composite::get_brokers,
    instrument::{get_all_instruments, get_instrument_by_id},
};

use super::{internal_error, json_response};
use crate::api::errors::ErrorResponse;

#[utoipa::path(
    get,
    path = "/api/instruments",
    tag = "market",
    responses(
        (status = 200, description = "All known instruments", body = [crate::database::models::instrument::Instrument]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn instruments() -> Result<impl IntoResponse, ErrorResponse> {
    let instruments = get_all_instruments()
        .await
        .map_err(|e| internal_error("InstrumentRetrievalError", e))?;
    json_response(&instruments).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/instruments/{isin}",
    tag = "market",
    params(("isin" = String, Path, description = "ISIN")),
    responses(
        (status = 200, description = "Instrument", body = crate::database::models::instrument::Instrument),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Instrument not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn instrument(Path(isin): Path<String>) -> Result<impl IntoResponse, ErrorResponse> {
    match get_instrument_by_id(&isin)
        .await
        .map_err(|e| internal_error("InstrumentRetrievalError", e))?
    {
        Some(instrument) => json_response(&instrument)
            .map_err(|status| internal_error("SerializationError", status)),
        None => Err(ErrorResponse::new(
            StatusCode::NOT_FOUND,
            "InstrumentNotFound",
            &format!("No instrument found for ISIN {isin}"),
            None,
        )),
    }
}

#[utoipa::path(
    get,
    path = "/api/brokers",
    tag = "market",
    responses(
        (status = 200, description = "Distinct broker names", body = [String]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn brokers() -> Result<impl IntoResponse, ErrorResponse> {
    let brokers = get_brokers()
        .await
        .map_err(|e| internal_error("BrokerRetrievalError", e))?;
    json_response(&brokers).map_err(|status| internal_error("SerializationError", status))
}
