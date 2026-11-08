use axum::{
    extract::{ConnectInfo, FromRequest, Path, Request, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::Utc;
use serde::{de::DeserializeOwned, Serialize};
use std::net::SocketAddr;
use tower_cookies::Cookies;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::{
    api::{errors::ErrorResponse, routes::AppState},
    database::{
        db_client,
        models::auth::{AccountLinkView, AuthSession, AuthState, CurrentUser, SessionSummary},
    },
    services::auth::{
        challenges::{LoginRequest, ReauthRequest, VerificationRequest},
        credential_changes::{
            EmptyRequest, PasswordChangeRequest, PendingTotpRequest, TotpConfirmRequest,
            TotpStartResponse,
        },
        links::{InspectLinkRequest, PasswordResetRequest, RecoverRequest, RegisterRequest},
        rate_limit::{self, RatePurpose},
        session_store::{self, IssuedSession},
        views, AuthError,
    },
};

/// Auth/admin bodies have strict DTOs and retain the structured error envelope.
pub struct AuthJson<T>(pub T);
impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for AuthJson<T> {
    type Rejection = ErrorResponse;
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|error| {
                ErrorResponse::new(
                    error.status(),
                    "InvalidRequest",
                    "Invalid authentication request.",
                    None,
                )
            })
    }
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct EnrollmentConfirmed {
    pub auth: AuthState,
    pub recovery_codes: Vec<String>,
}
#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct RecoveryCodesResponse {
    pub recovery_codes: Vec<String>,
}
#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct SessionsResponse {
    pub sessions: Vec<SessionSummary>,
}

pub async fn issued_state(
    state: &AppState,
    cookies: &Cookies,
    issued: IssuedSession,
) -> Result<AuthState, AuthError> {
    let db = db_client().await.map_err(|_| AuthError::Unavailable)?;
    let auth = views::state(&db, &issued.session, Utc::now()).await?;
    cookies.add(
        state
            .auth
            .session_cookie(issued.raw_cookie, issued.session.absolute_expires_at),
    );
    Ok(auth)
}

#[utoipa::path(get,path="/api/auth/state",tag="auth",responses((status=200,body=AuthState),(status=409,body=ErrorResponse),(status=503,body=ErrorResponse)))]
pub async fn auth_state(
    State(state): State<AppState>,
    cookies: Cookies,
    source: Option<Extension<AuthSession>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<AuthState>, AuthError> {
    rate_limit::enforce(RatePurpose::Anonymous, "", peer.ip()).await?;
    if let Some(Extension(source)) = source {
        let db = db_client().await.map_err(|_| AuthError::Unavailable)?;
        return Ok(Json(views::state(&db, &source, Utc::now()).await?));
    }
    // A stale read must never replace a rotated successor with an anonymous cookie.
    // Only a genuinely cookie-less browser may create initial authority.
    if cookies.get(crate::services::auth::COOKIE_NAME).is_some() {
        return Err(AuthError::StateChanged);
    }
    let mut db = db_client().await.map_err(|_| AuthError::Unavailable)?;
    let tx = db.transaction().await?;
    let issued = session_store::create_anonymous(&tx, Utc::now()).await?;
    let auth = views::state(&tx, &issued.session, Utc::now()).await?;
    tx.commit().await?;
    cookies.add(
        state
            .auth
            .session_cookie(issued.raw_cookie, issued.session.absolute_expires_at),
    );
    Ok(Json(auth))
}

#[utoipa::path(post,path="/api/auth/login",tag="auth",request_body=LoginRequest,responses((status=200,body=AuthState),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse),(status=503,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn login(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<LoginRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .login(&source, input.username, input.password, peer.ip())
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[utoipa::path(post,path="/api/auth/mfa/verify",tag="auth",request_body=VerificationRequest,responses((status=200,body=AuthState),(status=401,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn mfa_verify(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<VerificationRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .verify_mfa(&source, &input.code, input.code_type, peer.ip())
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[utoipa::path(post,path="/api/auth/totp/start",tag="auth",request_body=EmptyRequest,responses((status=200,body=TotpStartResponse),(status=403,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn totp_start(
    State(state): State<AppState>,
    Extension(source): Extension<AuthSession>,
    actor: Option<Extension<CurrentUser>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(_input): AuthJson<EmptyRequest>,
) -> Result<Json<TotpStartResponse>, AuthError> {
    let (_, enrollment) = state
        .auth
        .totp_start(&source, actor.as_ref().map(|a| &a.0), peer.ip())
        .await?;
    Ok(Json(enrollment))
}

#[utoipa::path(post,path="/api/auth/totp/confirm",tag="auth",request_body=TotpConfirmRequest,responses((status=200,body=EnrollmentConfirmed),(status=401,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn totp_confirm(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    actor: Option<Extension<CurrentUser>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<TotpConfirmRequest>,
) -> Result<Json<EnrollmentConfirmed>, AuthError> {
    let (issued, recovery_codes) = state
        .auth
        .totp_confirm(
            &source,
            actor.as_ref().map(|a| &a.0),
            &input.enrollment_id,
            &input.code,
            peer.ip(),
        )
        .await?;
    let auth = issued_state(&state, &cookies, issued).await?;
    Ok(Json(EnrollmentConfirmed {
        auth,
        recovery_codes,
    }))
}

#[utoipa::path(delete,path="/api/auth/totp/pending",tag="auth",request_body=PendingTotpRequest,responses((status=200,body=AuthState)),security(("browser_session"=[])))]
pub async fn totp_cancel(
    State(state): State<AppState>,
    Extension(source): Extension<AuthSession>,
    actor: Option<Extension<CurrentUser>>,
    AuthJson(input): AuthJson<PendingTotpRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let source = state
        .auth
        .totp_cancel(&source, actor.as_ref().map(|a| &a.0), &input.enrollment_id)
        .await?;
    let db = db_client().await.map_err(|_| AuthError::Unavailable)?;
    Ok(Json(views::state(&db, &source, Utc::now()).await?))
}

#[utoipa::path(post,path="/api/auth/reauth",tag="auth",request_body=ReauthRequest,responses((status=200,body=AuthState),(status=401,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn reauth(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    Extension(actor): Extension<CurrentUser>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<ReauthRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .reauthenticate(
            &actor,
            &source,
            input.password,
            &input.code,
            input.code_type,
            peer.ip(),
        )
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[utoipa::path(post,path="/api/auth/password/change",tag="auth",request_body=PasswordChangeRequest,responses((status=200,body=AuthState),(status=403,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn change_password(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    Extension(actor): Extension<CurrentUser>,
    AuthJson(input): AuthJson<PasswordChangeRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .change_password(&actor, &source, input.new_password)
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[utoipa::path(post,path="/api/auth/recovery-codes/regenerate",tag="auth",request_body=EmptyRequest,responses((status=200,body=RecoveryCodesResponse)),security(("browser_session"=[])))]
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    Extension(actor): Extension<CurrentUser>,
    AuthJson(_input): AuthJson<EmptyRequest>,
) -> Result<Json<RecoveryCodesResponse>, AuthError> {
    let (issued, recovery_codes) = state
        .auth
        .regenerate_recovery_codes(&actor, &source)
        .await?;
    issued_state(&state, &cookies, issued).await?;
    Ok(Json(RecoveryCodesResponse { recovery_codes }))
}

#[utoipa::path(post,path="/api/auth/logout",tag="auth",responses((status=204,description="Clears the cookie; absent or invalid sessions require only same-origin proof"),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn logout(
    State(state): State<AppState>,
    cookies: Cookies,
    source: Option<Extension<AuthSession>>,
) -> Result<StatusCode, AuthError> {
    if let Some(Extension(source)) = source {
        state.auth.logout(&source).await?;
    }
    cookies.add(state.auth.expired_cookie());
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get,path="/api/auth/sessions",tag="auth",responses((status=200,body=SessionsResponse)),security(("browser_session"=[])))]
pub async fn sessions(
    State(state): State<AppState>,
    Extension(source): Extension<AuthSession>,
    Extension(actor): Extension<CurrentUser>,
) -> Result<Json<SessionsResponse>, AuthError> {
    Ok(Json(SessionsResponse {
        sessions: state.auth.sessions(&actor, &source).await?,
    }))
}

#[utoipa::path(delete,path="/api/auth/sessions/{id}",tag="auth",params(("id"=String,Path)),responses((status=204),(status=404,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn revoke_session(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    Extension(actor): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, AuthError> {
    if state.auth.revoke_session(&actor, &source, &id).await? {
        cookies.add(state.auth.expired_cookie());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post,path="/api/auth/links/inspect",tag="auth",request_body=InspectLinkRequest,responses((status=200,body=AccountLinkView),(status=400,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn inspect_link(
    State(state): State<AppState>,
    Extension(source): Extension<AuthSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<InspectLinkRequest>,
) -> Result<Json<AccountLinkView>, AuthError> {
    let (_, link) = state
        .auth
        .inspect_link(&source, &input.token, peer.ip())
        .await?;
    Ok(Json(link))
}

#[utoipa::path(post,path="/api/auth/register",tag="auth",request_body=RegisterRequest,responses((status=200,body=AuthState),(status=400,body=ErrorResponse),(status=409,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn register(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<RegisterRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .register(
            &source,
            &input.link_id,
            input.username,
            input.password,
            peer.ip(),
        )
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[utoipa::path(post,path="/api/auth/password/reset",tag="auth",request_body=PasswordResetRequest,responses((status=200,body=AuthState),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn reset_password(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<PasswordResetRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .reset_password(
            &source,
            &input.link_id,
            input.new_password,
            &input.code,
            input.code_type,
            peer.ip(),
        )
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[utoipa::path(post,path="/api/auth/recover",tag="auth",request_body=RecoverRequest,responses((status=200,body=AuthState),(status=400,body=ErrorResponse)),security(("browser_session"=[])))]
pub async fn recover_account(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(source): Extension<AuthSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AuthJson(input): AuthJson<RecoverRequest>,
) -> Result<Json<AuthState>, AuthError> {
    let issued = state
        .auth
        .recover_account(&source, &input.link_id, input.new_password, peer.ip())
        .await?;
    Ok(Json(issued_state(&state, &cookies, issued).await?))
}

#[cfg(test)]
mod tests;
