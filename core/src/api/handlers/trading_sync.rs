use crate::{
    api::errors::ErrorResponse,
    database::models::auth::CurrentUser,
    services::{auth::{require_browser, AuthError}, trading::sync::{self, T212SyncError, T212SyncStatus, T212SyncRunResponse, reviews::{self, T212ReviewPage, T212ReviewRequest, T212ReviewResponse}}},
};
use axum::{extract::{Json, Query, rejection::{JsonRejection, QueryRejection}}, http::StatusCode, response::{IntoResponse, Response}, routing::{get, post, put}, Extension, Router};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulerInput { pub enabled: bool }

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunInput { #[serde(default)] pub full_history: bool }

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewQuery { pub batch_id: String, pub cursor: Option<i32>, pub limit: Option<usize> }


#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyQuery {}
fn invalid_input() -> Response {
    ErrorResponse::new(StatusCode::BAD_REQUEST, "TradingSyncInvalidReview", "Invalid sync request fields or pagination.", None).into_response()
}

fn failure(error: anyhow::Error) -> Response {
    if let Some(sync) = error.downcast_ref::<T212SyncError>() {
        let status = match sync.code {
            "TradingSyncUnavailable" | "TradingSyncAlreadyRunning" | "TradingSyncReviewRequired" | "TradingSyncReviewChanged" => StatusCode::CONFLICT,
            "TradingSyncInvalidReview" => StatusCode::BAD_REQUEST,
            _ => return internal_failure(),
        };
        return ErrorResponse::new(status, sync.code, sync.message, None).into_response();
    }
    if let Ok(auth) = error.downcast::<AuthError>() { return auth.into_response(); }
    internal_failure()
}

fn internal_failure() -> Response {
    ErrorResponse::new(StatusCode::INTERNAL_SERVER_ERROR, "TradingSyncFailed", "Trading 212 sync request could not be completed.", None).into_response()
}

fn pagination(input: &ReviewQuery) -> Result<usize, Response> {
    let limit = input.limit.unwrap_or(50);
    if input.batch_id.trim().is_empty() || input.cursor.is_some_and(|cursor| cursor < 0) || !(1..=200).contains(&limit) {
        return Err(invalid_input());
    }
    Ok(limit)
}

#[utoipa::path(get, path = "/api/trading/sync", tag = "trading", responses((status = 200, body = T212SyncStatus), (status = 403, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn status(Extension(current): Extension<CurrentUser>, query: Result<Query<EmptyQuery>, QueryRejection>) -> Result<Json<T212SyncStatus>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    query.map_err(|_| invalid_input())?;
    sync::status(&current.id).await.map(Json).map_err(failure)
}
#[utoipa::path(put, path = "/api/trading/sync/scheduler", tag = "trading", request_body = SchedulerInput, responses((status = 200, body = T212SyncStatus), (status = 400, body = ErrorResponse), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn scheduler(Extension(current): Extension<CurrentUser>, query: Result<Query<EmptyQuery>, QueryRejection>, input: Result<Json<SchedulerInput>, JsonRejection>) -> Result<Json<T212SyncStatus>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    query.map_err(|_| invalid_input())?;
    let Json(input) = input.map_err(|_| invalid_input())?;
    sync::save_schedule(&current.id, input.enabled).await.map(Json).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/sync/runs", tag = "trading", request_body = RunInput, responses((status = 202, body = T212SyncRunResponse), (status = 400, body = ErrorResponse), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn runs(Extension(current): Extension<CurrentUser>, query: Result<Query<EmptyQuery>, QueryRejection>, input: Result<Json<RunInput>, JsonRejection>) -> Result<(StatusCode, Json<T212SyncRunResponse>), Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    query.map_err(|_| invalid_input())?;
    let Json(input) = input.map_err(|_| invalid_input())?;
    sync::request_run(&current.id, input.full_history).await.map(|result| (StatusCode::ACCEPTED, Json(result))).map_err(failure)
}

#[utoipa::path(get, path = "/api/trading/sync/reviews", tag = "trading", params(("batch_id" = String, Query), ("cursor" = Option<i32>, Query, minimum = 0), ("limit" = Option<usize>, Query, minimum = 1, maximum = 200)), responses((status = 200, body = T212ReviewPage), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn review_list(Extension(current): Extension<CurrentUser>, input: Result<Query<ReviewQuery>, QueryRejection>) -> Result<Json<T212ReviewPage>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    let Query(input) = input.map_err(|_| invalid_input())?;
    let limit = pagination(&input)?;
    reviews::list(&current.id, &input.batch_id, input.cursor, limit).await.map(Json).map_err(failure)
}

#[utoipa::path(post, path = "/api/trading/sync/reviews", tag = "trading", request_body = T212ReviewRequest, responses((status = 200, body = T212ReviewResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)), security(("browser_session" = [])))]
pub async fn review_resolve(Extension(current): Extension<CurrentUser>, query: Result<Query<EmptyQuery>, QueryRejection>, input: Result<Json<T212ReviewRequest>, JsonRejection>) -> Result<Json<T212ReviewResponse>, Response> {
    require_browser(&current).map_err(IntoResponse::into_response)?;
    query.map_err(|_| invalid_input())?;
    let Json(input) = input.map_err(|_| invalid_input())?;
    if !(1..=200).contains(&input.decisions.len()) { return Err(invalid_input()); }
    reviews::resolve(&current.id, input.decisions).await.map(Json).map_err(failure)
}

pub fn router() -> Router {
    Router::new()
        .route("/trading/sync", get(status))
        .route("/trading/sync/scheduler", put(scheduler))
        .route("/trading/sync/runs", post(runs))
        .route("/trading/sync/reviews", get(review_list).post(review_resolve))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::auth::{CredentialKind, Role, UserId};

    #[tokio::test]
    async fn sync_management_requires_browser_before_accessing_storage() {
        for credential in [CredentialKind::PersonalReadToken, CredentialKind::PersonalWriteToken] {
            let actor = CurrentUser { id: UserId::new(), username: "owner".into(), role: Role::Admin, auth_version: 1, credential };
            assert_eq!(status(Extension(actor.clone()), Ok(Query(EmptyQuery {}))).await.unwrap_err().status(), StatusCode::FORBIDDEN);
            assert_eq!(scheduler(Extension(actor.clone()), Ok(Query(EmptyQuery {})), Ok(Json(SchedulerInput { enabled: true }))).await.unwrap_err().status(), StatusCode::FORBIDDEN);
            assert_eq!(runs(Extension(actor.clone()), Ok(Query(EmptyQuery {})), Ok(Json(RunInput { full_history: false }))).await.unwrap_err().status(), StatusCode::FORBIDDEN);
            assert_eq!(review_list(Extension(actor.clone()), Ok(Query(ReviewQuery { batch_id: "batch".into(), cursor: None, limit: None }))).await.unwrap_err().status(), StatusCode::FORBIDDEN);
            assert_eq!(review_resolve(Extension(actor), Ok(Query(EmptyQuery {})), Ok(Json(T212ReviewRequest { decisions: vec![] }))).await.unwrap_err().status(), StatusCode::FORBIDDEN);
        }
    }

    #[test]
    fn pagination_is_bounded_and_request_fields_are_strict() {
        let mut query = ReviewQuery { batch_id: "batch".into(), cursor: None, limit: None };
        assert_eq!(pagination(&query).unwrap(), 50);
        for limit in [0, 201, usize::MAX] {
            query.limit = Some(limit);
            assert_eq!(pagination(&query).unwrap_err().status(), StatusCode::BAD_REQUEST);
        }
        query.limit = Some(200);
        query.cursor = Some(-1);
        assert_eq!(pagination(&query).unwrap_err().status(), StatusCode::BAD_REQUEST);
        assert!(serde_json::from_str::<SchedulerInput>(r#"{"enabled":true,"user_id":"other"}"#).is_err());
        assert!(serde_json::from_str::<SchedulerInput>("{}").is_err());
        assert!(serde_json::from_str::<RunInput>(r#"{"full_history":null}"#).is_err());
        assert!(serde_json::from_str::<RunInput>(r#"{"owner":"other"}"#).is_err());
        assert!(!serde_json::from_str::<RunInput>("{}").unwrap().full_history);
    }

    #[tokio::test]
    async fn errors_preserve_safe_status_codes_without_exposing_internal_details() {
        for code in ["TradingSyncUnavailable", "TradingSyncAlreadyRunning", "TradingSyncReviewRequired", "TradingSyncReviewChanged", "TradingSyncInvalidReview"] {
            let response = failure(T212SyncError { code, message: "Safe message." }.into());
            assert_eq!(response.status(), if code == "TradingSyncInvalidReview" { StatusCode::BAD_REQUEST } else { StatusCode::CONFLICT });
        }
        assert_eq!(failure(AuthError::NotFound.into()).status(), StatusCode::NOT_FOUND);
        let response = failure(anyhow::anyhow!("secret signed URL and CSV"));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["message"], "Trading 212 sync request could not be completed.");
    }
}
