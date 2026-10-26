use axum::{extract::rejection::JsonRejection, http::StatusCode, response::IntoResponse, Json};

use super::{internal_error, json_response};
use crate::{
    api::errors::ErrorResponse,
    database::{db_client, queries::application_settings},
    services::shared::{
        logger,
        settings::{parse_log_level, ApplicationSettings},
    },
};

#[utoipa::path(
    get, path = "/api/settings", tag = "settings",
    responses(
        (status = 200, description = "Non-sensitive persisted application settings", body = ApplicationSettings),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("api_token" = []))
)]
pub async fn get_settings() -> Result<impl IntoResponse, ErrorResponse> {
    let client = db_client()
        .await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let settings = application_settings::read(&client)
        .await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&settings).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put, path = "/api/settings", tag = "settings", request_body = ApplicationSettings,
    responses(
        (status = 200, description = "Saved settings; log filter applies immediately in this API process", body = ApplicationSettings),
        (status = 400, description = "Unsupported log level", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Malformed settings payload", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("api_token" = []))
)]
pub async fn save_settings(
    payload: Result<Json<ApplicationSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(settings) = payload.map_err(|error| {
        ErrorResponse::new(error.status(), "InvalidRequest", &error.body_text(), None)
    })?;
    parse_log_level(&settings.log_level).map_err(|error| {
        ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "InvalidLogLevel",
            &error.to_string(),
            None,
        )
    })?;
    let client = db_client()
        .await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    application_settings::update(&client, &settings)
        .await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let settings = application_settings::read(&client)
        .await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    logger::apply_level(&settings.log_level)
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&settings).map_err(|status| internal_error("SerializationError", status))
}
