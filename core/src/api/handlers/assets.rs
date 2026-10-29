use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    api::errors::ErrorResponse,
    database::models::asset::{
        CreateAssetBalanceSnapshotRequest, CreateAssetRequest, CreateAssetTradeRequest,
        CreateAssetTransactionRequest, CreateAssetValuationRequest, CreateLinkedInterestRequest,
        UpdateAssetBalanceSnapshotRequest, UpdateAssetRequest, UpdateAssetTradeRequest,
        UpdateAssetTransactionRequest, UpdateAssetValuationRequest,
    },
    services::assets,
};

#[derive(Debug, Deserialize, IntoParams)]
pub struct AssetQuery {
    #[serde(default)]
    pub include_archived: bool,
}

fn json<T: serde::Serialize>(status: StatusCode, value: T) -> Response {
    (status, Json(value)).into_response()
}

fn invalid(error: impl ToString) -> ErrorResponse {
    ErrorResponse::new(
        StatusCode::BAD_REQUEST,
        "InvalidAssetRequest",
        &error.to_string(),
        None,
    )
}

fn internal(error: impl ToString) -> ErrorResponse {
    ErrorResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "AssetRequestFailed",
        &error.to_string(),
        None,
    )
}

fn not_found(resource: &str) -> ErrorResponse {
    ErrorResponse::new(
        StatusCode::NOT_FOUND,
        "AssetResourceNotFound",
        &format!("{resource} not found"),
        None,
    )
}

#[utoipa::path(
    get,
    path = "/api/assets",
    tag = "assets",
    params(AssetQuery),
    responses((status = 200, body = [crate::database::models::asset::AssetListRecord])),
    security(("api_token" = []))
)]
pub async fn list_assets(Query(query): Query<AssetQuery>) -> Result<Response, ErrorResponse> {
    assets::list_assets(query.include_archived)
        .await
        .map(|assets| json(StatusCode::OK, assets))
        .map_err(internal)
}

#[utoipa::path(
    post,
    path = "/api/assets",
    tag = "assets",
    request_body = crate::database::models::asset::CreateAssetRequest,
    responses((status = 201, body = crate::database::models::asset::Asset)),
    security(("api_token" = []))
)]
pub async fn create_asset(
    Json(request): Json<CreateAssetRequest>,
) -> Result<Response, ErrorResponse> {
    assets::create_asset(&request)
        .await
        .map(|asset| json(StatusCode::CREATED, asset))
        .map_err(invalid)
}

#[utoipa::path(
    get,
    path = "/api/assets/{asset_id}",
    tag = "assets",
    params(("asset_id" = String, Path)),
    responses(
        (status = 200, body = crate::database::models::asset::AssetDetailRecord),
        (status = 404, body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn get_asset(Path(asset_id): Path<String>) -> Result<Response, ErrorResponse> {
    assets::get_asset_detail(&asset_id)
        .await
        .map_err(internal)?
        .map(|asset| json(StatusCode::OK, asset))
        .ok_or_else(|| not_found("asset"))
}

#[utoipa::path(
    put,
    path = "/api/assets/{asset_id}",
    tag = "assets",
    params(("asset_id" = String, Path)),
    request_body = crate::database::models::asset::UpdateAssetRequest,
    responses((status = 200, body = crate::database::models::asset::Asset)),
    security(("api_token" = []))
)]
pub async fn update_asset(
    Path(asset_id): Path<String>,
    Json(request): Json<UpdateAssetRequest>,
) -> Result<Response, ErrorResponse> {
    assets::update_asset(&asset_id, &request)
        .await
        .map(|asset| json(StatusCode::OK, asset))
        .map_err(invalid)
}

#[utoipa::path(
    delete,
    path = "/api/assets/{asset_id}",
    tag = "assets",
    params(("asset_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn archive_asset(Path(asset_id): Path<String>) -> Result<Response, ErrorResponse> {
    assets::archive_asset(&asset_id)
        .await
        .map(|_| StatusCode::NO_CONTENT.into_response())
        .map_err(invalid)
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/restore",
    tag = "assets",
    params(("asset_id" = String, Path)),
    responses((status = 200, body = crate::database::models::asset::Asset)),
    security(("api_token" = []))
)]
pub async fn restore_asset(Path(asset_id): Path<String>) -> Result<Response, ErrorResponse> {
    assets::restore_asset(&asset_id)
        .await
        .map(|asset| json(StatusCode::OK, asset))
        .map_err(invalid)
}

#[utoipa::path(
    get,
    path = "/api/assets/holdings",
    tag = "assets",
    responses((status = 200, body = [crate::database::models::asset::CustomAssetHolding])),
    security(("api_token" = []))
)]
pub async fn holdings() -> Result<Response, ErrorResponse> {
    assets::calculate_custom_holdings()
        .await
        .map(|holdings| json(StatusCode::OK, holdings))
        .map_err(internal)
}

#[utoipa::path(
    get,
    path = "/api/cash-accounts/summary",
    tag = "assets",
    responses((status = 200, body = crate::database::models::asset::WeightedCashInterestSummary)),
    security(("api_token" = []))
)]
pub async fn cash_summary() -> Result<Response, ErrorResponse> {
    assets::calculate_weighted_cash_interest()
        .await
        .map(|summary| json(StatusCode::OK, summary))
        .map_err(internal)
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/trades",
    tag = "assets",
    params(("asset_id" = String, Path)),
    request_body = crate::database::models::asset::CreateAssetTradeRequest,
    responses((status = 201, body = crate::database::models::asset::AssetTrade)),
    security(("api_token" = []))
)]
pub async fn create_trade(
    Path(asset_id): Path<String>,
    Json(request): Json<CreateAssetTradeRequest>,
) -> Result<Response, ErrorResponse> {
    assets::create_asset_trade(&asset_id, &request)
        .await
        .map(|trade| json(StatusCode::CREATED, trade))
        .map_err(invalid)
}

#[utoipa::path(
    put,
    path = "/api/assets/{asset_id}/trades/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    request_body = crate::database::models::asset::UpdateAssetTradeRequest,
    responses((status = 200, body = crate::database::models::asset::AssetTrade)),
    security(("api_token" = []))
)]
pub async fn update_trade(
    Path((asset_id, trade_id)): Path<(String, String)>,
    Json(request): Json<UpdateAssetTradeRequest>,
) -> Result<Response, ErrorResponse> {
    assets::update_asset_trade(&asset_id, &trade_id, &request)
        .await
        .map_err(invalid)?
        .map(|trade| json(StatusCode::OK, trade))
        .ok_or_else(|| not_found("trade"))
}

#[utoipa::path(
    delete,
    path = "/api/assets/{asset_id}/trades/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn delete_trade(
    Path((asset_id, trade_id)): Path<(String, String)>,
) -> Result<Response, ErrorResponse> {
    match assets::delete_asset_trade(&asset_id, &trade_id)
        .await
        .map_err(invalid)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(not_found("trade")),
    }
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/valuations",
    tag = "assets",
    params(("asset_id" = String, Path)),
    request_body = crate::database::models::asset::CreateAssetValuationRequest,
    responses((status = 201, body = crate::database::models::asset::AssetValuation)),
    security(("api_token" = []))
)]
pub async fn create_valuation(
    Path(asset_id): Path<String>,
    Json(request): Json<CreateAssetValuationRequest>,
) -> Result<Response, ErrorResponse> {
    assets::create_asset_valuation(&asset_id, &request)
        .await
        .map(|valuation| json(StatusCode::CREATED, valuation))
        .map_err(invalid)
}

#[utoipa::path(
    put,
    path = "/api/assets/{asset_id}/valuations/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    request_body = crate::database::models::asset::UpdateAssetValuationRequest,
    responses((status = 200, body = crate::database::models::asset::AssetValuation)),
    security(("api_token" = []))
)]
pub async fn update_valuation(
    Path((asset_id, valuation_id)): Path<(String, String)>,
    Json(request): Json<UpdateAssetValuationRequest>,
) -> Result<Response, ErrorResponse> {
    assets::update_asset_valuation(&asset_id, &valuation_id, &request)
        .await
        .map_err(invalid)?
        .map(|valuation| json(StatusCode::OK, valuation))
        .ok_or_else(|| not_found("valuation"))
}

#[utoipa::path(
    delete,
    path = "/api/assets/{asset_id}/valuations/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn delete_valuation(
    Path((asset_id, valuation_id)): Path<(String, String)>,
) -> Result<Response, ErrorResponse> {
    match assets::delete_asset_valuation(&asset_id, &valuation_id)
        .await
        .map_err(invalid)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(not_found("valuation")),
    }
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/transactions",
    tag = "assets",
    params(("asset_id" = String, Path)),
    request_body = crate::database::models::asset::CreateAssetTransactionRequest,
    responses((status = 201, body = crate::database::models::asset::AssetTransaction)),
    security(("api_token" = []))
)]
pub async fn create_transaction(
    Path(asset_id): Path<String>,
    Json(request): Json<CreateAssetTransactionRequest>,
) -> Result<Response, ErrorResponse> {
    assets::create_asset_transaction(&asset_id, &request)
        .await
        .map(|transaction| json(StatusCode::CREATED, transaction))
        .map_err(invalid)
}

#[utoipa::path(
    put,
    path = "/api/assets/{asset_id}/transactions/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    request_body = crate::database::models::asset::UpdateAssetTransactionRequest,
    responses((status = 200, body = crate::database::models::asset::AssetTransaction)),
    security(("api_token" = []))
)]
pub async fn update_transaction(
    Path((asset_id, transaction_id)): Path<(String, String)>,
    Json(request): Json<UpdateAssetTransactionRequest>,
) -> Result<Response, ErrorResponse> {
    assets::update_asset_transaction(&asset_id, &transaction_id, &request)
        .await
        .map_err(invalid)?
        .map(|transaction| json(StatusCode::OK, transaction))
        .ok_or_else(|| not_found("transaction"))
}

#[utoipa::path(
    delete,
    path = "/api/assets/{asset_id}/transactions/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn delete_transaction(
    Path((asset_id, transaction_id)): Path<(String, String)>,
) -> Result<Response, ErrorResponse> {
    match assets::delete_asset_transaction(&asset_id, &transaction_id)
        .await
        .map_err(invalid)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(not_found("transaction")),
    }
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/balance-snapshots",
    tag = "assets",
    params(("asset_id" = String, Path)),
    request_body = crate::database::models::asset::CreateAssetBalanceSnapshotRequest,
    responses((status = 201, body = crate::database::models::asset::AssetBalanceSnapshot)),
    security(("api_token" = []))
)]
pub async fn create_balance_snapshot(
    Path(asset_id): Path<String>,
    Json(request): Json<CreateAssetBalanceSnapshotRequest>,
) -> Result<Response, ErrorResponse> {
    assets::create_asset_balance_snapshot(&asset_id, &request)
        .await
        .map(|snapshot| json(StatusCode::CREATED, snapshot))
        .map_err(invalid)
}

#[utoipa::path(
    put,
    path = "/api/assets/{asset_id}/balance-snapshots/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    request_body = crate::database::models::asset::UpdateAssetBalanceSnapshotRequest,
    responses((status = 200, body = crate::database::models::asset::AssetBalanceSnapshot)),
    security(("api_token" = []))
)]
pub async fn update_balance_snapshot(
    Path((asset_id, snapshot_id)): Path<(String, String)>,
    Json(request): Json<UpdateAssetBalanceSnapshotRequest>,
) -> Result<Response, ErrorResponse> {
    assets::update_asset_balance_snapshot(&asset_id, &snapshot_id, &request)
        .await
        .map_err(invalid)?
        .map(|snapshot| json(StatusCode::OK, snapshot))
        .ok_or_else(|| not_found("balance snapshot"))
}

#[utoipa::path(
    delete,
    path = "/api/assets/{asset_id}/balance-snapshots/{record_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("record_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn delete_balance_snapshot(
    Path((asset_id, snapshot_id)): Path<(String, String)>,
) -> Result<Response, ErrorResponse> {
    match assets::delete_asset_balance_snapshot(&asset_id, &snapshot_id)
        .await
        .map_err(invalid)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(not_found("balance snapshot")),
    }
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/interest",
    tag = "assets",
    params(("asset_id" = String, Path)),
    request_body = crate::database::models::asset::CreateLinkedInterestRequest,
    responses((status = 201, body = crate::database::models::asset::LinkedAssetInterest)),
    security(("api_token" = []))
)]
pub async fn create_interest(
    Path(asset_id): Path<String>,
    Json(request): Json<CreateLinkedInterestRequest>,
) -> Result<Response, ErrorResponse> {
    assets::create_linked_interest(&asset_id, &request)
        .await
        .map(|interest| json(StatusCode::CREATED, interest))
        .map_err(invalid)
}

#[utoipa::path(
    put,
    path = "/api/assets/{asset_id}/interest/{interest_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("interest_id" = String, Path)),
    request_body = crate::database::models::asset::CreateLinkedInterestRequest,
    responses((status = 200, body = crate::database::models::asset::LinkedAssetInterest)),
    security(("api_token" = []))
)]
pub async fn update_interest(
    Path((asset_id, interest_id)): Path<(String, String)>,
    Json(request): Json<CreateLinkedInterestRequest>,
) -> Result<Response, ErrorResponse> {
    assets::update_linked_interest(&asset_id, &interest_id, &request)
        .await
        .map_err(invalid)?
        .map(|interest| json(StatusCode::OK, interest))
        .ok_or_else(|| not_found("manual linked interest"))
}

#[utoipa::path(
    post,
    path = "/api/assets/{asset_id}/interest/{interest_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("interest_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn link_interest(
    Path((asset_id, interest_id)): Path<(String, String)>,
) -> Result<Response, ErrorResponse> {
    match assets::link_interest(&asset_id, &interest_id)
        .await
        .map_err(invalid)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(invalid("interest is already linked to another asset")),
    }
}

#[utoipa::path(
    delete,
    path = "/api/assets/{asset_id}/interest/{interest_id}",
    tag = "assets",
    params(("asset_id" = String, Path), ("interest_id" = String, Path)),
    responses((status = 204)),
    security(("api_token" = []))
)]
pub async fn unlink_interest(
    Path((asset_id, interest_id)): Path<(String, String)>,
) -> Result<Response, ErrorResponse> {
    match assets::unlink_interest(&asset_id, &interest_id)
        .await
        .map_err(invalid)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(not_found("linked interest")),
    }
}
