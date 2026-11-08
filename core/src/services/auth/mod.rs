pub mod admin;
pub mod api_tokens;
pub mod challenges;
pub mod credential_changes;
pub mod links;
pub mod maintenance;
pub mod operators;
pub mod password;
pub mod rate_limit;
pub mod session_store;
pub mod totp;
pub mod views;

use std::{fmt, sync::Arc};

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use tower_cookies::{
    cookie::{time::Duration, SameSite},
    Cookie,
};

use crate::{
    api::errors::ErrorResponse,
    database::models::auth::{AuthSession, CredentialKind, CurrentUser, UserId},
    services::shared::{env::get_env_variable, secrets, urls::parse_public_origin},
};

pub const COOKIE_NAME: &str = "doughbox_session";
pub const RECENCY_SECONDS: i64 = 5 * 60;

#[derive(Debug)]
pub enum AuthError {
    Unavailable,
    InvalidCredentials,
    InvalidVerificationCode,
    Unauthenticated,
    TwoFactorRequired,
    Forbidden,
    CsrfRejected,
    ReauthenticationRequired,
    StateChanged,
    RateLimited(u64),
    InvalidInput(&'static str),
    UsernameUnavailable,
    InvalidAccountLink,
    LastAdministrator,
    NotFound,
    AmbiguousCredentials,
}

impl AuthError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::InvalidCredentials | Self::InvalidVerificationCode | Self::Unauthenticated => {
                StatusCode::UNAUTHORIZED
            }
            Self::TwoFactorRequired
            | Self::Forbidden
            | Self::CsrfRejected
            | Self::ReauthenticationRequired => StatusCode::FORBIDDEN,
            Self::StateChanged | Self::UsernameUnavailable | Self::LastAdministrator => {
                StatusCode::CONFLICT
            }
            Self::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::InvalidInput(_) | Self::InvalidAccountLink | Self::AmbiguousCredentials => {
                StatusCode::BAD_REQUEST
            }
            Self::NotFound => StatusCode::NOT_FOUND,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "AuthenticationUnavailable",
            Self::InvalidCredentials => "InvalidCredentials",
            Self::InvalidVerificationCode => "InvalidVerificationCode",
            Self::Unauthenticated => "Unauthenticated",
            Self::TwoFactorRequired => "TwoFactorRequired",
            Self::Forbidden => "Forbidden",
            Self::CsrfRejected => "CsrfRejected",
            Self::ReauthenticationRequired => "ReauthenticationRequired",
            Self::StateChanged => "AuthenticationStateChanged",
            Self::RateLimited(_) => "AuthenticationRateLimited",
            Self::InvalidInput(_) => "InvalidRequest",
            Self::UsernameUnavailable => "UsernameUnavailable",
            Self::InvalidAccountLink => "InvalidAccountLink",
            Self::LastAdministrator => "LastAdministrator",
            Self::NotFound => "NotFound",
            Self::AmbiguousCredentials => "AmbiguousCredentials",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::Unavailable => "Authentication is temporarily unavailable.",
            Self::InvalidCredentials => "Invalid username or password.",
            Self::InvalidVerificationCode => "Invalid or expired verification code.",
            Self::Unauthenticated => "Authentication is required.",
            Self::TwoFactorRequired => "Complete two-factor authentication first.",
            Self::Forbidden => "This credential is not authorized for this operation.",
            Self::CsrfRejected => "The request's origin or CSRF token is invalid.",
            Self::ReauthenticationRequired => "Verify your password and second factor again.",
            Self::StateChanged => "Authentication state changed. Reload before trying again.",
            Self::RateLimited(_) => "Too many authentication attempts. Try again later.",
            Self::InvalidInput(message) => message,
            Self::UsernameUnavailable => "That username is unavailable.",
            Self::InvalidAccountLink => "This account link is invalid or expired.",
            Self::LastAdministrator => "At least one active, enrolled administrator is required.",
            Self::NotFound => "The requested resource was not found.",
            Self::AmbiguousCredentials => {
                "Use either a session cookie or a bearer token, not both."
            }
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for AuthError {}

impl From<tokio_postgres::Error> for AuthError {
    fn from(_: tokio_postgres::Error) -> Self {
        Self::Unavailable
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let mut response =
            ErrorResponse::new(self.status(), self.code(), self.message(), None).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        let retry_after = match self {
            Self::Unavailable => Some(1),
            Self::RateLimited(seconds) => Some(seconds.max(1)),
            _ => None,
        };
        if let Some(seconds) = retry_after {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

// No Debug: the master key lives once per process, never in request logs.
pub struct AuthConfig {
    pub public_origin: String,
    pub secure_cookie: bool,
    master_key: [u8; 32],
}

impl AuthConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let origin = get_env_variable("DOUGHBOX_PUBLIC_URL").ok_or_else(|| {
            anyhow::anyhow!("DOUGHBOX_PUBLIC_URL must be an absolute HTTP(S) origin")
        })?;
        let key = get_env_variable("AUTH_MASTER_KEY").ok_or_else(|| {
            anyhow::anyhow!("AUTH_MASTER_KEY must contain exactly 64 hexadecimal characters")
        })?;
        let allow_http = get_env_variable("AUTH_ALLOW_INSECURE_HTTP").as_deref() == Some("true");
        Self::parse(&origin, &key, allow_http)
    }

    pub fn parse(origin: &str, encoded_key: &str, allow_http: bool) -> anyhow::Result<Self> {
        let public_origin = parse_public_origin(origin)
            .ok_or_else(|| anyhow::anyhow!("DOUGHBOX_PUBLIC_URL must be an absolute HTTP(S) origin without credentials, a path, query, or fragment"))?;
        let secure_cookie = public_origin.starts_with("https://");
        anyhow::ensure!(secure_cookie || allow_http, "DOUGHBOX_PUBLIC_URL requires HTTPS; AUTH_ALLOW_INSECURE_HTTP=true is only for local/trusted networks");
        if !secure_cookie {
            tracing::warn!(
                "Authentication uses insecure HTTP; use only on a local/trusted network"
            );
        }
        let master_key = secrets::parse_master_key(encoded_key).map_err(|_| {
            anyhow::anyhow!("AUTH_MASTER_KEY must contain exactly 64 hexadecimal characters")
        })?;
        Ok(Self {
            public_origin,
            secure_cookie,
            master_key,
        })
    }
}

pub struct AuthService {
    pub config: AuthConfig,
    pub passwords: password::PasswordService,
}

impl AuthService {
    pub fn new(config: AuthConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            passwords: password::PasswordService::new(),
        })
    }

    pub fn encrypt_totp(
        &self,
        user_id: &UserId,
        pending: bool,
        secret: &[u8],
    ) -> Result<String, AuthError> {
        let aad = totp_aad(user_id, pending);
        secrets::encrypt(secret, &self.config.master_key, aad.as_bytes())
            .map(|encoded| format!("v1:{encoded}"))
            .map_err(|_| AuthError::Unavailable)
    }

    pub fn decrypt_totp(
        &self,
        user_id: &UserId,
        pending: bool,
        ciphertext: &str,
    ) -> Result<Vec<u8>, AuthError> {
        let envelope = ciphertext
            .strip_prefix("v1:")
            .ok_or(AuthError::Unavailable)?;
        let aad = totp_aad(user_id, pending);
        secrets::decrypt(envelope, &self.config.master_key, aad.as_bytes())
            .map_err(|_| AuthError::Unavailable)
    }

    pub fn session_cookie(&self, raw: String, expires_at: DateTime<Utc>) -> Cookie<'static> {
        Cookie::build((COOKIE_NAME, raw))
            .http_only(true)
            .secure(self.config.secure_cookie)
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(Duration::seconds(
                (expires_at - Utc::now()).num_seconds().max(0),
            ))
            .build()
    }

    pub fn expired_cookie(&self) -> Cookie<'static> {
        Cookie::build((COOKIE_NAME, ""))
            .http_only(true)
            .secure(self.config.secure_cookie)
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(Duration::ZERO)
            .build()
    }

    pub fn check_origin(&self, headers: &axum::http::HeaderMap) -> Result<(), AuthError> {
        if headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site") {
            return Err(AuthError::CsrfRejected);
        }
        let origin = headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_public_origin)
            .ok_or(AuthError::CsrfRejected)?;
        if origin != self.config.public_origin {
            return Err(AuthError::CsrfRejected);
        }
        Ok(())
    }
}

fn totp_aad(user_id: &UserId, pending: bool) -> String {
    format!(
        "doughbox-totp-v1:{}:{}",
        user_id.as_str(),
        if pending { "pending" } else { "active" }
    )
}

pub fn random_credential(prefix: &str) -> Result<String, AuthError> {
    let mut bytes = [0_u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| AuthError::Unavailable)?;
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub fn digest(context: &str, value: &str) -> String {
    secrets::encode_hex(blake3::derive_key(context, value.as_bytes()).into_iter())
}

pub fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

pub fn require_browser(actor: &CurrentUser) -> Result<(), AuthError> {
    if actor.credential != CredentialKind::BrowserSession {
        return Err(AuthError::Forbidden);
    }
    Ok(())
}

pub fn require_recent(session: &AuthSession, now: DateTime<Utc>) -> Result<(), AuthError> {
    if session.stage != crate::database::models::auth::SessionStage::Authenticated
        || !session
            .reauthenticated_at
            .is_some_and(|time| time <= now && (now - time).num_seconds() <= RECENCY_SECONDS)
    {
        return Err(AuthError::ReauthenticationRequired);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
