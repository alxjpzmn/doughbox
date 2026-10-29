use axum::{extract::rejection::JsonRejection, http::StatusCode, response::IntoResponse, Json};

use super::{internal_error, json_response};
use crate::{
    api::errors::ErrorResponse,
    database::{db_client, queries::{application_settings, telegram_schedule}},
    services::shared::{
        logger,
        settings::{parse_log_level, ApplicationSettings, ApplicationSettingsStatus, TelegramReportScheduleSettings},
    },
};

#[utoipa::path(
    get, path = "/api/settings", tag = "settings",
    responses(
        (status = 200, description = "Non-sensitive application settings and Telegram configuration presence; never returns credentials", body = ApplicationSettingsStatus),
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
    let status = ApplicationSettingsStatus::load(settings).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&status)
        .map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put, path = "/api/settings", tag = "settings", request_body = ApplicationSettings,
    responses(
        (status = 200, description = "Saved settings and Telegram configuration presence; log filter applies immediately in this API process", body = ApplicationSettingsStatus),
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
    let status = ApplicationSettingsStatus::load(settings).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&status)
        .map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put, path = "/api/settings/telegram-scheduler", tag = "settings",
    request_body = TelegramReportScheduleSettings,
    responses(
        (status = 200, description = "Saved daily UTC Telegram schedule and full application settings", body = ApplicationSettingsStatus),
        (status = 400, description = "Invalid UTC send time", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Malformed schedule payload", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("api_token" = []))
)]
pub async fn save_telegram_scheduler(
    payload: Result<Json<TelegramReportScheduleSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(settings) = payload.map_err(|error| {
        ErrorResponse::new(error.status(), "InvalidRequest", &error.body_text(), None)
    })?;
    // Reject malformed times before making any database call.
    telegram_schedule::parse_send_time(&settings.send_time_utc).map_err(|error| {
        ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidSendTime", &error.to_string(), None)
    })?;
    let client = db_client().await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    telegram_schedule::update_schedule(&client, settings.enabled, &settings.send_time_utc).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let settings = application_settings::read(&client).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let status = ApplicationSettingsStatus::load(settings).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&status).map_err(|status| internal_error("SerializationError", status))
}
