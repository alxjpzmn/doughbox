use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use tower_sessions::Session;
use utoipa::ToSchema;

use crate::services::shared::{constants::SESSION_TOKEN_KEY, env::get_env_variable};

#[derive(Deserialize, ToSchema)]
pub struct LoginRequestData {
    pub password: String,
}

pub async fn issue_session_cookie(session: Session) -> anyhow::Result<(), StatusCode> {
    session
        .insert(SESSION_TOKEN_KEY, "user")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[utoipa::path(
    post,
    path = "/api/login",
    tag = "auth",
    request_body = LoginRequestData,
    responses(
        (status = 200, description = "Session cookie issued"),
        (status = 401, description = "Invalid password")
    )
)]
pub async fn login(
    session: Session,
    Json(payload): Json<LoginRequestData>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let password = get_env_variable("PASSWORD");
    match password {
        Some(password) if payload.password == password => {
            issue_session_cookie(session).await?;
            Ok(StatusCode::OK)
        }
        None => {
            issue_session_cookie(session).await?;
            Ok(StatusCode::OK)
        }
        _ => Ok(StatusCode::UNAUTHORIZED),
    }
}

#[utoipa::path(
    post,
    path = "/api/logout",
    tag = "auth",
    responses((status = 200, description = "Session deleted"))
)]
pub async fn logout(session: Session) -> anyhow::Result<impl IntoResponse, StatusCode> {
    session
        .delete()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

#[utoipa::path(
    get,
    path = "/api/auth_state",
    tag = "auth",
    responses(
        (status = 200, description = "Authenticated", body = String),
        (status = 401, description = "Unauthorized")
    ),
    security(("api_token" = []))
)]
pub async fn auth_state() -> impl IntoResponse {
    (StatusCode::OK, "authenticated")
}
