use super::super::errors::ErrorResponse;
use crate::services::trading::{self, ConnectInput, Proposal, TradingState};
use axum::{
    extract::{Json, Path},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Router,
};
use serde_json::{json, Value};

fn failure(error: anyhow::Error) -> ErrorResponse {
    ErrorResponse::new(
        StatusCode::CONFLICT,
        "TradingRequestFailed",
        &error.to_string(),
        None,
    )
}
fn mutation(headers: &HeaderMap) -> Result<(), ErrorResponse> {
    // JSON-only mutations cannot be submitted by cross-site HTML forms. Agent Bearer
    // credentials are deliberately not accepted by the owner authentication layer.
    if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next() == Some("application/json"))
        && headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) != Some("cross-site")
    {
        Ok(())
    } else {
        Err(ErrorResponse::new(
            StatusCode::FORBIDDEN,
            "TradingOriginRejected",
            "Use a same-origin JSON request for trading changes.",
            None,
        ))
    }
}
#[utoipa::path(get, path = "/api/trading", tag = "trading", responses((status = 200, body = TradingState)), security(("api_token" = [])))]
pub async fn state() -> Result<Json<TradingState>, ErrorResponse> {
    trading::state().await.map(Json).map_err(|_| {
        ErrorResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "TradingUnavailable",
            "Trading state could not be loaded.",
            None,
        )
    })
}
#[utoipa::path(put, path = "/api/trading/connection", tag = "trading", request_body = ConnectInput, responses((status = 200, body = TradingState), (status = 409, body = ErrorResponse)), security(("api_token" = [])))]
pub async fn connect(
    headers: HeaderMap,
    Json(input): Json<ConnectInput>,
) -> Result<Json<TradingState>, ErrorResponse> {
    mutation(&headers)?;
    trading::connect(input).await.map(Json).map_err(failure)
}
#[utoipa::path(delete, path = "/api/trading/connection", tag = "trading", responses((status = 200, body = TradingState)), security(("api_token" = [])))]
pub async fn disconnect(headers: HeaderMap) -> Result<Json<TradingState>, ErrorResponse> {
    mutation(&headers)?;
    trading::disconnect().await.map(Json).map_err(failure)
}
#[utoipa::path(post, path = "/api/trading/agent-token", tag = "trading", responses((status = 200, body = Value)), security(("api_token" = [])))]
pub async fn token(headers: HeaderMap) -> Result<Json<Value>, ErrorResponse> {
    mutation(&headers)?;
    trading::rotate_token()
        .await
        .map(|token| Json(json!({"token":token})))
        .map_err(failure)
}
#[utoipa::path(post, path = "/api/trading/proposals/{id}/approve", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse)), security(("api_token" = [])))]
pub async fn approve(
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Proposal>, ErrorResponse> {
    mutation(&headers)?;
    trading::approve(id).await.map(Json).map_err(failure)
}
#[utoipa::path(post, path = "/api/trading/proposals/{id}/discard", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse)), security(("api_token" = [])))]
pub async fn discard(
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Proposal>, ErrorResponse> {
    mutation(&headers)?;
    trading::discard(&id).await.map(Json).map_err(failure)
}
pub fn router() -> Router {
    Router::new()
        .route("/trading", get(state))
        .route("/trading/connection", put(connect).delete(disconnect))
        .route("/trading/agent-token", post(token))
        .route("/trading/proposals/{id}/approve", post(approve))
        .route("/trading/proposals/{id}/discard", post(discard))
}
