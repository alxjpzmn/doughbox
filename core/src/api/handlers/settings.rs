use axum::{extract::rejection::JsonRejection, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::Utc;

use super::{internal_error, json_response};
use crate::{
    api::errors::ErrorResponse,
    database::{
        db_client,
        models::auth::{AuthSession, CredentialKind, CurrentUser, Role},
        queries::{application_settings, auth, telegram_schedule},
    },
    services::{
        auth::{require_browser, require_recent, session_store, AuthError},
        notifications,
        shared::{
            logger,
            settings::{parse_log_level, ApplicationSettings, ApplicationSettingsStatus, TelegramNotificationSettings, TelegramReportScheduleSettings},
        },
    },
};

fn auth_error(error: AuthError) -> ErrorResponse {
    ErrorResponse::new(error.status(), error.code(), error.message(), None)
}

fn require_private_write(current: &CurrentUser) -> Result<(), ErrorResponse> {
    if current.credential == CredentialKind::PersonalReadToken {
        return Err(auth_error(AuthError::Forbidden));
    }
    Ok(())
}

#[utoipa::path(
    get, path = "/api/settings", tag = "settings",
    responses(
        (status = 200, description = "Own notification destination and daily schedule; never includes instance log controls", body = ApplicationSettingsStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("browser_session" = []), ("api_token" = []))
)]
pub async fn get_settings(
    Extension(current): Extension<CurrentUser>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let status = ApplicationSettingsStatus::load(&current.id).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&status).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get, path = "/api/admin/settings", tag = "settings",
    responses(
        (status = 200, description = "Instance log controls; browser administrators only", body = ApplicationSettings),
        (status = 403, description = "Administrator browser session required", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("browser_session" = []))
)]
pub async fn get_admin_settings(
    Extension(current): Extension<CurrentUser>,
) -> Result<impl IntoResponse, ErrorResponse> {
    require_browser(&current).map_err(auth_error)?;
    if current.role != Role::Admin { return Err(auth_error(AuthError::Forbidden)); }
    let client = db_client().await.map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let settings = application_settings::read(&client).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&settings).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put, path = "/api/admin/settings", tag = "settings", request_body = ApplicationSettings,
    responses(
        (status = 200, description = "Saved instance log controls; filter applies immediately in this API process", body = ApplicationSettings),
        (status = 400, description = "Unsupported log level", body = ErrorResponse),
        (status = 403, description = "Recent administrator browser session required", body = ErrorResponse),
        (status = 422, description = "Malformed settings payload", body = ErrorResponse),
        (status = 503, description = "Authentication store unavailable", body = ErrorResponse)
    ), security(("browser_session" = []))
)]
pub async fn save_settings(
    Extension(current): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    payload: Result<Json<ApplicationSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    require_browser(&current).map_err(auth_error)?;
    if source.user_id.as_ref() != Some(&current.id) || source.auth_version != Some(current.auth_version) {
        return Err(auth_error(AuthError::StateChanged));
    }
    let Json(settings) = payload.map_err(|error| {
        ErrorResponse::new(error.status(), "InvalidRequest", &error.body_text(), None)
    })?;
    parse_log_level(&settings.log_level).map_err(|error| {
        ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidLogLevel", &error.to_string(), None)
    })?;
    let mut client = db_client().await.map_err(|_| auth_error(AuthError::Unavailable))?;
    let transaction = client.transaction().await.map_err(|_| auth_error(AuthError::Unavailable))?;
    auth::lock_actor(&transaction, &current, true).await.map_err(auth_error)?;
    let locked = session_store::lock_source(&transaction, &source, Utc::now()).await.map_err(auth_error)?;
    require_recent(&locked, Utc::now()).map_err(auth_error)?;
    application_settings::update(&transaction, &settings).await
        .map_err(|_| auth_error(AuthError::Unavailable))?;
    let settings = application_settings::read(&transaction).await
        .map_err(|_| auth_error(AuthError::Unavailable))?;
    transaction.commit().await.map_err(|_| auth_error(AuthError::Unavailable))?;
    logger::apply_level(&settings.log_level)
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&settings).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put, path = "/api/settings/telegram", tag = "settings", request_body = TelegramNotificationSettings,
    responses(
        (status = 200, description = "Saved own notification destination and schedule", body = ApplicationSettingsStatus),
        (status = 400, description = "Invalid chat destination", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Malformed destination payload", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("browser_session" = []), ("api_token" = []))
)]
pub async fn save_telegram(
    Extension(current): Extension<CurrentUser>,
    payload: Result<Json<TelegramNotificationSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    require_private_write(&current)?;
    let Json(settings) = payload.map_err(|error| {
        ErrorResponse::new(error.status(), "InvalidRequest", &error.body_text(), None)
    })?;
    let chat_id = settings.normalized_chat_id().map_err(|error| {
        ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidTelegramChatId", &error.to_string(), None)
    })?;
    let client = db_client().await.map_err(|e| internal_error("ApplicationSettingsError", e))?;
    notifications::set_chat_id(&current.id, &client, chat_id.as_deref()).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let status = ApplicationSettingsStatus::load(&current.id).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&status).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put, path = "/api/settings/telegram-scheduler", tag = "settings",
    request_body = TelegramReportScheduleSettings,
    responses(
        (status = 200, description = "Saved own daily UTC Telegram schedule and notification destination", body = ApplicationSettingsStatus),
        (status = 400, description = "Invalid UTC send time", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Malformed schedule payload", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ), security(("browser_session" = []), ("api_token" = []))
)]
pub async fn save_telegram_scheduler(
    Extension(current): Extension<CurrentUser>,
    payload: Result<Json<TelegramReportScheduleSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    require_private_write(&current)?;
    let Json(settings) = payload.map_err(|error| {
        ErrorResponse::new(error.status(), "InvalidRequest", &error.body_text(), None)
    })?;
    // Reject malformed times before making any database call.
    telegram_schedule::parse_send_time(&settings.send_time_utc).map_err(|error| {
        ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidSendTime", &error.to_string(), None)
    })?;
    let client = db_client().await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    telegram_schedule::update_schedule(&current.id, &client, settings.enabled, &settings.send_time_utc).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    let status = ApplicationSettingsStatus::load(&current.id).await
        .map_err(|e| internal_error("ApplicationSettingsError", e))?;
    json_response(&status).map_err(|status| internal_error("SerializationError", status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::auth::UserId;

    fn current(role: Role, credential: CredentialKind) -> CurrentUser {
        CurrentUser {
            id: UserId::new(),
            username: "alice".into(),
            role,
            auth_version: 1,
            credential,
        }
    }

    #[test]
    fn private_notification_mutations_reject_read_tokens_not_owner_write_authority() {
        assert!(require_private_write(&current(Role::Member, CredentialKind::BrowserSession)).is_ok());
        assert!(require_private_write(&current(Role::Member, CredentialKind::PersonalWriteToken)).is_ok());
        for role in [Role::Admin, Role::Member] {
            let error = require_private_write(&current(role, CredentialKind::PersonalReadToken)).err().unwrap();
            assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn instance_settings_require_admin_browser_before_database_access() {
        for (role, credential) in [
            (Role::Member, CredentialKind::BrowserSession),
            (Role::Admin, CredentialKind::PersonalReadToken),
            (Role::Admin, CredentialKind::PersonalWriteToken),
        ] {
            let error = get_admin_settings(Extension(current(role, credential))).await.err().unwrap();
            assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
        }
    }
}
