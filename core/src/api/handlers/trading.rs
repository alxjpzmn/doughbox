use super::super::errors::ErrorResponse;
use crate::{
    database::models::auth::{AuthSession, CurrentUser},
    services::{auth::{require_browser, AuthError}, trading::{self, ConnectInput, Proposal, TradingState}},
};
use axum::{
    extract::{Json, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Router,
};
use serde_json::{json, Value};

fn failure(error: anyhow::Error) -> Response {
    match error.downcast::<AuthError>() {
        Ok(auth) => auth.into_response(),
        Err(error) => ErrorResponse::new(
            StatusCode::CONFLICT,
            "TradingRequestFailed",
            &error.to_string(),
            None,
        ).into_response(),
    }
}

#[utoipa::path(get, path = "/api/trading", tag = "trading", responses((status = 200, body = TradingState)), security(("browser_session" = [])))]
pub async fn state(Extension(current): Extension<CurrentUser>) -> Result<Json<TradingState>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::state(&current.id).await.map(Json).map_err(|_| {
        ErrorResponse::new(StatusCode::INTERNAL_SERVER_ERROR, "TradingUnavailable", "Trading state could not be loaded.", None).into_response()
    })
}

#[utoipa::path(put, path = "/api/trading/connection", tag = "trading", request_body = ConnectInput, responses((status = 200, body = TradingState), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn connect(
    Extension(current): Extension<CurrentUser>,
    Json(input): Json<ConnectInput>,
) -> Result<Json<TradingState>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::connect(&current.id, input).await.map(Json).map_err(failure)
}

#[utoipa::path(delete, path = "/api/trading/connection", tag = "trading", responses((status = 200, body = TradingState)), security(("browser_session" = [])))]
pub async fn disconnect(Extension(current): Extension<CurrentUser>) -> Result<Json<TradingState>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::disconnect(&current.id).await.map(Json).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/agent-token", tag = "trading", responses((status = 200, body = Value)), security(("browser_session" = [])))]
pub async fn token(
    Extension(current): Extension<CurrentUser>,
    source: Option<Extension<AuthSession>>,
) -> Result<Json<Value>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    let source = source.ok_or_else(|| AuthError::Unauthenticated.into_response())?.0;
    trading::rotate_token(&current.id, &current, &source).await
        .map(|token| Json(json!({"token":token}))).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/proposals/{id}/approve", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn approve(
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Proposal>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::approve(&current.id, id).await.map(Json).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/proposals/{id}/discard", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn discard(
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Proposal>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::discard(&current.id, &id).await.map(Json).map_err(failure)
}

pub fn router() -> Router {
    Router::new()
        .route("/trading", get(state))
        .route("/trading/connection", put(connect).delete(disconnect))
        .route("/trading/agent-token", post(token))
        .route("/trading/proposals/{id}/approve", post(approve))
        .route("/trading/proposals/{id}/discard", post(discard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trading_preserves_authentication_errors_and_retry_headers() {
        use axum::body::to_bytes;
        for auth in [AuthError::ReauthenticationRequired, AuthError::StateChanged, AuthError::NotFound, AuthError::Unavailable] {
            let expected = auth.status();
            let code = auth.code();
            let response = failure(auth.into());
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()["cache-control"], "no-store");
            if expected == StatusCode::SERVICE_UNAVAILABLE {
                assert_eq!(response.headers()["retry-after"], "1");
            }
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"], code);
        }
    }

    #[tokio::test]
    async fn personal_token_cannot_issue_agent_credentials_without_a_browser_source() {
        use crate::database::models::auth::{CredentialKind, Role, UserId};
        let mut actor = CurrentUser {
            id: UserId::new(),
            username: "alice".into(),
            role: Role::Member,
            auth_version: 1,
            credential: CredentialKind::PersonalWriteToken,
        };
        let response = token(Extension(actor.clone()), None).await.err().unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = state(Extension(actor.clone())).await.err().unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        actor.credential = CredentialKind::BrowserSession;
        let response = token(Extension(actor), None).await.err().unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
