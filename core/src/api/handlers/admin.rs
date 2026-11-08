use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use tower_cookies::Cookies;

use crate::{
    api::{errors::ErrorResponse, routes::AppState},
    database::models::auth::{AccountStatus, AuthSession, CurrentUser, UserSummary},
    services::auth::{
        admin::{InvitesResponse, UserPatchRequest, UsersResponse},
        credential_changes::EmptyRequest,
        links::IssuedAccountLink,
        AuthError,
    },
};

use super::auth::AuthJson;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/users", get(list_users))
        .route("/admin/users/{id}", axum::routing::patch(update_user))
        .route(
            "/admin/users/{id}/revoke-sessions",
            post(revoke_user_sessions),
        )
        .route(
            "/admin/users/{id}/password-reset-link",
            post(password_reset_link),
        )
        .route("/admin/invites", get(list_invites).post(create_invite))
        .route("/admin/invites/{id}", delete(revoke_invite))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

/// Requires a full administrator browser session; never exposes portfolio data.
#[utoipa::path(get, path="/api/admin/users", tag="admin", responses((status=200,body=UsersResponse),(status=403,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn list_users(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
) -> Result<Json<UsersResponse>, AuthError> {
    Ok(Json(state.auth.list_users(&actor, &source).await?))
}

/// Requires a recent two-factor proof. Effective self-changes log out the actor.
#[utoipa::path(patch, path="/api/admin/users/{id}", tag="admin", params(("id"=String,Path)), request_body=UserPatchRequest, responses((status=200,body=UserSummary),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn update_user(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    Path(id): Path<String>,
    AuthJson(input): AuthJson<UserPatchRequest>,
) -> Result<Json<UserSummary>, AuthError> {
    let summary = state.auth.update_user(&actor, &source, &id, input).await?;
    if summary.id == actor.id.as_str()
        && (summary.role != actor.role || summary.status != AccountStatus::Active)
    {
        cookies.add(state.auth.expired_cookie());
    }
    Ok(Json(summary))
}

/// Revoke the target's browser and automation credentials; recent administrator proof required.
#[utoipa::path(post, path="/api/admin/users/{id}/revoke-sessions", tag="admin", params(("id"=String,Path)), request_body=EmptyRequest, responses((status=204),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn revoke_user_sessions(
    State(state): State<AppState>,
    cookies: Cookies,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    Path(id): Path<String>,
    AuthJson(_input): AuthJson<EmptyRequest>,
) -> Result<StatusCode, AuthError> {
    state
        .auth
        .revoke_user_sessions(&actor, &source, &id)
        .await?;
    if id == actor.id.as_str() {
        cookies.add(state.auth.expired_cookie());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Issue a one-hour reset link without changing or bypassing the target's existing factor.
#[utoipa::path(post, path="/api/admin/users/{id}/password-reset-link", tag="admin", params(("id"=String,Path)), request_body=EmptyRequest, responses((status=200,body=IssuedAccountLink),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn password_reset_link(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    Path(id): Path<String>,
    AuthJson(_input): AuthJson<EmptyRequest>,
) -> Result<Json<IssuedAccountLink>, AuthError> {
    Ok(Json(
        state.auth.password_reset_link(&actor, &source, &id).await?,
    ))
}

/// Lists invitation metadata only, never the bearer URL or token.
#[utoipa::path(get, path="/api/admin/invites", tag="admin", responses((status=200,body=InvitesResponse),(status=403,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn list_invites(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
) -> Result<Json<InvitesResponse>, AuthError> {
    Ok(Json(state.auth.list_invites(&actor, &source).await?))
}

/// Issue a seven-day member invitation once; recent administrator proof required.
#[utoipa::path(post, path="/api/admin/invites", tag="admin", request_body=EmptyRequest, responses((status=200,body=IssuedAccountLink),(status=403,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn create_invite(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    AuthJson(_input): AuthJson<EmptyRequest>,
) -> Result<Json<IssuedAccountLink>, AuthError> {
    Ok(Json(state.auth.create_invite(&actor, &source).await?))
}

/// Invalidate an unconsumed invitation; recent administrator proof required.
#[utoipa::path(delete, path="/api/admin/invites/{id}", tag="admin", params(("id"=String,Path)), responses((status=204),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("browser_session"=[])))]
pub async fn revoke_invite(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    Path(id): Path<String>,
) -> Result<StatusCode, AuthError> {
    state.auth.revoke_invite(&actor, &source, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
