use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{delete, get},
    Extension, Json, Router,
};

use crate::{
    api::{errors::ErrorResponse, routes::AppState},
    database::models::auth::{AuthSession, CurrentUser},
    services::auth::{
        api_tokens::{ApiTokensResponse, CreateApiTokenRequest, IssuedApiToken},
        AuthError,
    },
};

use super::auth::AuthJson;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/api-tokens", get(list).post(create))
        .route("/auth/api-tokens/{id}", delete(revoke))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[utoipa::path(
    get, path = "/api/auth/api-tokens", tag = "auth",
    responses(
        (status = 200, description = "Own unrevoked, unexpired personal-token summaries; no plaintext tokens", body = ApiTokensResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, description = "Full browser session required", body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    security(("browser_session" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
) -> Result<Json<ApiTokensResponse>, AuthError> {
    Ok(Json(ApiTokensResponse {
        tokens: state.auth.api_tokens(&actor, &source).await?,
    }))
}

#[utoipa::path(
    post, path = "/api/auth/api-tokens", tag = "auth", request_body = CreateApiTokenRequest,
    responses(
        (status = 200, description = "Personal token, shown only in this committed response; expires in 90 days", body = IssuedApiToken),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, description = "Recently reauthenticated full browser session required", body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    security(("browser_session" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    AuthJson(input): AuthJson<CreateApiTokenRequest>,
) -> Result<Json<IssuedApiToken>, AuthError> {
    Ok(Json(
        state
            .auth
            .create_api_token(&actor, &source, input.name, input.scope)
            .await?,
    ))
}

#[utoipa::path(
    delete, path = "/api/auth/api-tokens/{id}", tag = "auth",
    params(("id" = String, Path, description = "Own personal-token ID, not its plaintext credential")),
    responses(
        (status = 204),
        (status = 401, body = ErrorResponse),
        (status = 403, description = "Recently reauthenticated full browser session required", body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    security(("browser_session" = []))
)]
pub async fn revoke(
    State(state): State<AppState>,
    Extension(actor): Extension<CurrentUser>,
    Extension(source): Extension<AuthSession>,
    Path(id): Path<String>,
) -> Result<StatusCode, AuthError> {
    state.auth.revoke_api_token(&actor, &source, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
