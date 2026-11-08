pub mod admin;
pub mod api_tokens;
pub mod assets;
pub mod auth;
pub mod data_imports;
pub mod market;
pub mod performance;
pub mod portfolio;
pub mod settings;
pub mod statement_imports;
pub mod taxation;
pub mod timeline;
pub mod trades;
pub mod trading;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use chrono::{DateTime, NaiveDate, Utc};
use tower_cookies::Cookies;

use crate::{
    api::routes::AppState,
    database::{db_client, models::auth::{CredentialKind, Role, SessionStage}, queries::auth as auth_queries},
    services::auth::{constant_time_eq, digest, session_store, AuthError, COOKIE_NAME},
};

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

pub(super) fn security_accounting_error(error: anyhow::Error, fallback_code: &str) -> ErrorResponse {
    if let Some(gap) = error.downcast_ref::<crate::services::performance::SecurityHistoryGap>() {
        ErrorResponse::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "IncompleteSecurityHistory",
            &gap.to_string(),
            None,
        )
    } else {
        internal_error(fallback_code, error)
    }
}

/// The common browser/token boundary runs before JSON or multipart extraction.
pub async fn check_auth(
    State(state): State<AppState>,
    cookies: Cookies,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let path = request.extensions().get::<axum::extract::OriginalUri>()
        .map(|original| original.0.path()).unwrap_or_else(|| request.uri().path());
    let auth_route = path.starts_with("/api/auth/");
    let cookie = cookies.get(COOKIE_NAME);
    let authorization = request.headers().get(axum::http::header::AUTHORIZATION);
    if cookie.is_some() && authorization.is_some() { return Err(AuthError::AmbiguousCredentials); }
    let unsafe_request = !matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS);
    let db = db_client().await.map_err(|_| AuthError::Unavailable)?;
    let now = Utc::now();
    let session = match cookie {
        Some(cookie) => session_store::load(&db, cookie.value(), now).await?,
        None => None,
    };
    let current = if let Some(header) = authorization {
        if auth_route { return Err(AuthError::Forbidden); }
        let token = header.to_str().ok().and_then(|v| v.strip_prefix("Bearer ")).ok_or(AuthError::Unauthenticated)?;
        let encoded = token.strip_prefix("doughbox_api_").ok_or(AuthError::Unauthenticated)?;
        if encoded.len() != 43 || !encoded.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b,b'-'|b'_')) {
            return Err(AuthError::Unauthenticated);
        }
        let hash = digest("doughbox-auth-api-v1", token);
        let row = db.query_opt(
            "SELECT u.*,t.scope AS token_scope FROM auth_api_tokens t JOIN auth_users u ON u.id=t.user_id WHERE t.token_hash=$1 AND t.revoked_at IS NULL AND t.expires_at>$2 AND t.auth_version=u.auth_version AND u.status='active' AND u.username IS NOT NULL AND u.password_hash IS NOT NULL AND u.totp_secret_encrypted IS NOT NULL",
            &[&hash,&now],
        ).await?.ok_or(AuthError::Unauthenticated)?;
        let credential = match row.get::<_,&str>("token_scope") { "read"=>CredentialKind::PersonalReadToken, "write"=>CredentialKind::PersonalWriteToken,_=>return Err(AuthError::Unavailable) };
        Some(auth_queries::user_from_row(row)?.current(credential).ok_or(AuthError::Unauthenticated)?)
    } else if let Some(session) = &session {
        if session.stage == SessionStage::Authenticated {
            let user = auth_queries::by_id(session.user_id.as_ref().ok_or(AuthError::Unauthenticated)?, &db).await?.ok_or(AuthError::Unauthenticated)?;
            if session.auth_version != Some(user.auth_version) { return Err(AuthError::Unauthenticated); }
            Some(user.current(CredentialKind::BrowserSession).ok_or(AuthError::Unauthenticated)?)
        } else { None }
    } else { None };
    if unsafe_request && authorization.is_none() {
        state.auth.check_origin(request.headers())?;
        if let Some(source) = session.as_ref() {
            let csrf = request.headers().get("x-csrf-token").and_then(|v|v.to_str().ok()).ok_or(AuthError::CsrfRejected)?;
            if !constant_time_eq(csrf,&source.csrf_token) { return Err(AuthError::CsrfRejected); }
        } else if path != "/api/auth/logout" || request.method() != axum::http::Method::POST {
            // Only explicit signout may clear an absent/invalid source without CSRF.
            return Err(AuthError::CsrfRejected);
        }
    }
    let restricted_auth = matches!(path,
        "/api/auth/state" | "/api/auth/login" | "/api/auth/mfa/verify" | "/api/auth/totp/start"
        | "/api/auth/totp/confirm" | "/api/auth/totp/pending" | "/api/auth/logout"
        | "/api/auth/links/inspect" | "/api/auth/register" | "/api/auth/password/reset" | "/api/auth/recover");
    if !auth_route || !restricted_auth {
        let actor = current.as_ref().ok_or_else(|| if session.is_some() { AuthError::TwoFactorRequired } else { AuthError::Unauthenticated })?;
        let global = path.starts_with("/api/admin/") || path == "/api/admin"
            || path == "/api/data-imports" || path.starts_with("/api/data-imports/")
            || path == "/api/prices/scheduler" || path == "/api/prices/refresh" || path.starts_with("/api/prices/feeds/");
        if global && (actor.role != Role::Admin || actor.credential != CredentialKind::BrowserSession) { return Err(AuthError::Forbidden); }
        if (auth_route || path == "/api/trading" || path.starts_with("/api/trading/")) && actor.credential != CredentialKind::BrowserSession { return Err(AuthError::Forbidden); }
        if unsafe_request && actor.credential == CredentialKind::PersonalReadToken { return Err(AuthError::Forbidden); }
    }
    if let Some(source) = session { request.extensions_mut().insert(source); }
    if let Some(actor) = current { request.extensions_mut().insert(actor); }
    Ok(next.run(request).await)
}

/// Outermost API response policy also covers extractor/router/MCP rejections.
pub async fn response_policy(request: Request, next: Next) -> Response {
    let api = request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if api { response.headers_mut().insert(axum::http::header::CACHE_CONTROL, HeaderValue::from_static("no-store")); }
    response.headers_mut().insert(axum::http::header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}
