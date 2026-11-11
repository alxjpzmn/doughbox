use super::super::errors::ErrorResponse;
use crate::{
    database::models::auth::{AuthSession, CurrentUser},
    services::{auth::{require_browser, AuthError}, trading::{self, ConnectInput, Proposal, TradingState, provider::Instrument, pies::{self, PieAccess, PieApprovalInput, PieDetail, PieList, PieRateLimited, SubmitPieInput}}},
};
use axum::{
    body::to_bytes,
    extract::{Json, Path, Query, Request},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Router,
};
use serde_json::{json, Value};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentQuery { query: String }

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TradingRateLimitDetails { retry_after_seconds: u64 }

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TradingRateLimitResponse {
    status: u16,
    error: &'static str,
    message: String,
    details: TradingRateLimitDetails,
}

fn approval_input(bytes: &[u8]) -> Result<PieApprovalInput, Response> {
    if bytes.is_empty() { return Ok(PieApprovalInput::default()); }
    serde_json::from_slice(bytes).map_err(|_| ErrorResponse::new(
        StatusCode::BAD_REQUEST, "InvalidApproval", "Approval body must contain only an optional confirm_empty_pie boolean.", None,
    ).into_response())
}

fn failure(error: anyhow::Error) -> Response {
    if let Some(rate) = error.downcast_ref::<PieRateLimited>() {
        let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(TradingRateLimitResponse {
            status:429, error:"TradingRateLimited", message:rate.to_string(),
            details:TradingRateLimitDetails { retry_after_seconds:rate.retry_after_seconds },
        })).into_response();
        response.headers_mut().insert(header::RETRY_AFTER, rate.retry_after_seconds.to_string().parse().expect("integer header"));
        return response;
    }
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

#[utoipa::path(post, path = "/api/trading/proposals/{id}/approve", tag = "trading", params(("id" = String, Path)), request_body(content = PieApprovalInput, description = "Optional body for Pie deletion only. Zero bytes are accepted even with application/json; all other approvals require an empty body. Deletion requires confirm_empty_pie=true."), responses((status = 200, body = Proposal), (status = 400, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 429, body = TradingRateLimitResponse, headers(("Retry-After" = String, description = "Minimum wait in seconds.")))), security(("browser_session" = [])))]
pub async fn approve(
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Proposal>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    let bytes = to_bytes(request.into_body(), 4096).await.map_err(|_| ErrorResponse::new(
        StatusCode::PAYLOAD_TOO_LARGE, "InvalidApproval", "Approval body exceeds 4096 bytes.", None,
    ).into_response())?;
    if !bytes.is_empty() {
        let proposal = trading::browser_proposal(&current.id, &id).await.map_err(failure)?;
        if !matches!(proposal.content, trading::ProposalContent::Pie { pie } if matches!(pie.mutation, pies::PreparedPieMutation::Delete { .. })) {
            return Err(ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidApproval", "Only Pie deletion accepts an approval body.", None).into_response());
        }
    }
    trading::approve(&current.id, id, approval_input(&bytes)?).await.map(Json).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/proposals/{id}/discard", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn discard(
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Proposal>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::discard(&current.id, &id).await.map(Json).map_err(failure)
}

#[utoipa::path(get, path = "/api/trading/pies", tag = "trading", responses((status = 200, body = PieList), (status = 429, body = TradingRateLimitResponse, headers(("Retry-After" = String, description = "Minimum wait in seconds.")))), security(("browser_session" = [])))]
pub async fn pies_list(Extension(current): Extension<CurrentUser>) -> Result<Json<PieList>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    pies::list(&current.id, PieAccess::Browser).await.map(Json).map_err(failure)
}

#[utoipa::path(get, path = "/api/trading/pies/{id}", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = PieDetail), (status = 429, body = TradingRateLimitResponse, headers(("Retry-After" = String, description = "Minimum wait in seconds.")))), security(("browser_session" = [])))]
pub async fn pie_detail(Extension(current): Extension<CurrentUser>, Path(id): Path<String>) -> Result<Json<PieDetail>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    pies::detail(&current.id, PieAccess::Browser, &id).await.map(Json).map_err(failure)
}

#[utoipa::path(get, path = "/api/trading/account", tag = "trading", responses((status = 200, body = crate::services::trading::account::BrokerAccountState), (status = 429, body = TradingRateLimitResponse, headers(("Retry-After" = String, description = "Minimum wait in seconds.")))), security(("browser_session" = [])))]
pub async fn broker_account(Extension(current): Extension<CurrentUser>) -> Result<Json<crate::services::trading::account::BrokerAccountState>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::account::load(&current.id, PieAccess::Browser, false).await.map(Json).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/pie-proposals", tag = "trading", request_body = SubmitPieInput, responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse), (status = 429, body = TradingRateLimitResponse, headers(("Retry-After" = String, description = "Minimum wait in seconds.")))), security(("browser_session" = [])))]
pub async fn submit_pie(Extension(current): Extension<CurrentUser>, Json(input): Json<SubmitPieInput>) -> Result<Json<Proposal>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    pies::submit(&current.id, PieAccess::Browser, input).await.map(|outcome| Json(outcome.proposal)).map_err(failure)
}

/// Reads broker evidence only; never resubmits a mutation.
#[utoipa::path(post, path = "/api/trading/proposals/{id}/reconcile", tag = "trading", params(("id" = String, Path)), responses((status = 200, body = Proposal), (status = 409, body = ErrorResponse), (status = 429, body = TradingRateLimitResponse, headers(("Retry-After" = String, description = "Minimum wait in seconds.")))), security(("browser_session" = [])))]
pub async fn reconcile(Extension(current): Extension<CurrentUser>, Path(id): Path<String>) -> Result<Json<Proposal>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    pies::reconcile(&current.id, &id).await.map(Json).map_err(failure)
}

#[utoipa::path(get, path = "/api/trading/instruments", tag = "trading", params(("query" = String, Query, min_length = 1, max_length = 100)), responses((status = 200, body = [Instrument])), security(("browser_session" = [])))]
pub async fn instruments(Extension(current): Extension<CurrentUser>, Query(input): Query<InstrumentQuery>) -> Result<Json<Vec<Instrument>>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    trading::browser_search(&current.id, &input.query).await.map(Json).map_err(failure)
}

pub fn router() -> Router {
    Router::new()
        .route("/trading", get(state))
        .route("/trading/connection", put(connect).delete(disconnect))
        .route("/trading/agent-token", post(token))
        .route("/trading/pies", get(pies_list))
        .route("/trading/pies/{id}", get(pie_detail))
        .route("/trading/account", get(broker_account))
        .route("/trading/pie-proposals", post(submit_pie))
        .route("/trading/instruments", get(instruments))
        .route("/trading/proposals/{id}/reconcile", post(reconcile))
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

    #[test]
    fn optional_approval_bytes_are_strict_and_do_not_depend_on_content_type() {
        assert!(!approval_input(b"").unwrap().confirm_empty_pie);
        assert!(!approval_input(b"{}").unwrap().confirm_empty_pie);
        assert!(approval_input(br#"{"confirm_empty_pie":true}"#).unwrap().confirm_empty_pie);
        for invalid in [b"null".as_slice(), br#"{"confirm_empty_pie":null}"#, br#"{"confirm_empty_pie":"true"}"#, br#"{"execute":true}"#] {
            assert_eq!(approval_input(invalid).unwrap_err().status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn typed_pie_pacing_response_keeps_retry_information() {
        let response = failure(PieRateLimited { retry_after_seconds: 71 }.into());
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "71");
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"], "TradingRateLimited");
        assert_eq!(value["details"]["retry_after_seconds"], 71);
    }
}
