use axum::{
    extract::{rejection::JsonRejection, Path},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio_postgres::Client;
use utoipa::ToSchema;

use crate::database::{
    db_client,
    models::instrument::InstrumentDetails,
    queries::{
        background_job as jobs,
        composite::get_brokers,
        instrument::{get_all_instruments, get_instrument_by_id, rename_instrument},
        price, price_scheduler,
    },
};
use crate::services::market_data::prices::{
    self as pricing,
    config::Feed,
    types::{PriceSchedulerSettings, PricingStatus},
};

use super::{internal_error, json_response};
use crate::api::errors::ErrorResponse;

#[derive(Deserialize, ToSchema)]
pub struct RenameInstrumentRequest {
    /// Display name, trimmed before saving. Must not be blank.
    pub name: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ManualPriceRequest {
    /// Positive EUR price per unit, encoded as a decimal string to retain precision.
    #[serde(with = "rust_decimal::serde::str")]
    #[schema(value_type = String, example = "123.4567890123456789")]
    pub price: Decimal,
    /// RFC3339 valuation time, defaulting to now. Future timestamps are not accepted.
    pub as_of: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/api/instruments",
    tag = "market",
    responses(
        (status = 200, description = "All known instruments with safe current quotes, including unheld instruments", body = [InstrumentDetails]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn instruments() -> Result<impl IntoResponse, ErrorResponse> {
    let client = db_client()
        .await
        .map_err(|e| internal_error("InstrumentRetrievalError", e))?;
    let instruments = get_all_instruments(&client)
        .await
        .map_err(|e| internal_error("InstrumentRetrievalError", e))?;
    json_response(&instruments).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/instruments/{isin}",
    tag = "market",
    params(("isin" = String, Path, description = "ISIN")),
    responses(
        (status = 200, description = "Instrument with safe current quote and provenance", body = InstrumentDetails),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Instrument not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn instrument(Path(isin): Path<String>) -> Result<impl IntoResponse, ErrorResponse> {
    let client = db_client()
        .await
        .map_err(|e| internal_error("InstrumentRetrievalError", e))?;
    let instrument = required_instrument(&client, &isin).await?;
    json_response(&instrument).map_err(|status| internal_error("SerializationError", status))
}

fn instrument_not_found(isin: &str) -> ErrorResponse {
    ErrorResponse::new(
        StatusCode::NOT_FOUND,
        "InstrumentNotFound",
        &format!("No instrument found for ISIN {isin}"),
        None,
    )
}

fn invalid_json(rejection: JsonRejection) -> ErrorResponse {
    ErrorResponse::new(
        rejection.status(),
        "InvalidRequest",
        &rejection.body_text(),
        None,
    )
}

async fn required_instrument(
    client: &Client,
    isin: &str,
) -> Result<InstrumentDetails, ErrorResponse> {
    get_instrument_by_id(client, isin)
        .await
        .map_err(|e| internal_error("InstrumentRetrievalError", e))?
        .ok_or_else(|| instrument_not_found(isin))
}

#[utoipa::path(
    patch,
    path = "/api/instruments/{isin}",
    tag = "market",
    params(("isin" = String, Path, description = "Existing imported instrument ISIN")),
    request_body = RenameInstrumentRequest,
    responses(
        (status = 200, description = "Updated instrument; prices and pins are unchanged", body = InstrumentDetails),
        (status = 400, description = "Blank instrument name or malformed JSON", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Instrument not found", body = ErrorResponse),
        (status = 422, description = "Invalid request fields", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn rename(
    Path(isin): Path<String>,
    payload: Result<Json<RenameInstrumentRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(request) = payload.map_err(invalid_json)?;
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "InvalidInstrumentName",
            "Instrument name must not be blank",
            None,
        ));
    }
    let client = db_client()
        .await
        .map_err(|e| internal_error("InstrumentUpdateError", e))?;
    let instrument = rename_instrument(&client, &isin, name)
        .await
        .map_err(|e| internal_error("InstrumentUpdateError", e))?
        .ok_or_else(|| instrument_not_found(&isin))?;
    json_response(&instrument).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    put,
    path = "/api/instruments/{isin}/price",
    tag = "market",
    params(("isin" = String, Path, description = "Existing imported instrument ISIN")),
    request_body = ManualPriceRequest,
    responses(
        (status = 200, description = "Instrument with a pinned manual EUR unit price", body = InstrumentDetails),
        (status = 400, description = "Nonpositive price, future timestamp, or malformed JSON", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Instrument not found", body = ErrorResponse),
        (status = 422, description = "Invalid decimal string or RFC3339 timestamp", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn set_price(
    Path(isin): Path<String>,
    payload: Result<Json<ManualPriceRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(request) = payload.map_err(invalid_json)?;
    if request.price <= Decimal::ZERO {
        return Err(ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "InvalidManualPrice",
            "Manual EUR unit price must be positive",
            None,
        ));
    }
    let now = Utc::now();
    let as_of = request.as_of.unwrap_or(now);
    if as_of > now {
        return Err(ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "InvalidPriceTimestamp",
            "Manual price timestamp cannot be in the future",
            None,
        ));
    }
    let mut client = db_client()
        .await
        .map_err(|e| internal_error("PriceUpdateError", e))?;
    required_instrument(&client, &isin).await?;
    price::set_manual(&mut client, &isin, request.price, as_of)
        .await
        .map_err(|e| internal_error("PriceUpdateError", e))?;
    let instrument = required_instrument(&client, &isin).await?;
    json_response(&instrument).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    delete,
    path = "/api/instruments/{isin}/price",
    tag = "market",
    params(("isin" = String, Path, description = "Existing imported instrument ISIN")),
    responses(
        (status = 200, description = "Instrument after removing the pin and reselecting stored market prices or fallback", body = InstrumentDetails),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Instrument not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn unpin_price(Path(isin): Path<String>) -> Result<impl IntoResponse, ErrorResponse> {
    let mut client = db_client()
        .await
        .map_err(|e| internal_error("PriceUpdateError", e))?;
    required_instrument(&client, &isin).await?;
    price::unpin(&mut client, &isin)
        .await
        .map_err(|e| internal_error("PriceUpdateError", e))?;
    let instrument = required_instrument(&client, &isin).await?;
    json_response(&instrument).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/brokers",
    tag = "market",
    responses(
        (status = 200, description = "Distinct broker names", body = [String]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn brokers() -> Result<impl IntoResponse, ErrorResponse> {
    let brokers = get_brokers()
        .await
        .map_err(|e| internal_error("BrokerRetrievalError", e))?;
    json_response(&brokers).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/prices",
    tag = "market",
    responses(
        (status = 200, description = "Persistent price scheduler, queue, and current holding quote status", body = PricingStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn prices() -> Result<impl IntoResponse, ErrorResponse> {
    let status = pricing::status()
        .await
        .map_err(|e| internal_error("PriceStatusError", e))?;
    json_response(&status).map_err(|status| internal_error("SerializationError", status))
}

fn supported_feed(value: &str) -> Result<Feed, ErrorResponse> {
    Feed::from_key(value).map_err(|error| {
        ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "InvalidPriceFeed",
            &error.to_string(),
            None,
        )
    })
}

#[utoipa::path(
    put, path = "/api/prices/scheduler", tag = "market",
    request_body = PriceSchedulerSettings,
    responses(
        (status = 200, description = "Persisted scheduler settings and current status; enabling queues due feeds", body = PricingStatus),
        (status = 400, description = "Invalid interval, unsupported feed, or enabled scheduler without selected feeds", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Invalid request fields", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn save_scheduler(
    payload: Result<Json<PriceSchedulerSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(settings) = payload.map_err(invalid_json)?;
    if !(1..=1440).contains(&settings.interval_minutes)
        || !(1..=4).contains(&settings.worker_concurrency)
        || (settings.enabled && settings.feeds.is_empty())
    {
        return Err(ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidPriceSchedule",
            "Interval must be 1–1440 minutes and worker concurrency 1–4; enabling requires at least one feed", None));
    }
    let feeds = settings
        .feeds
        .iter()
        .map(|feed| supported_feed(feed))
        .collect::<Result<Vec<_>, _>>()?;
    let mut client = db_client()
        .await
        .map_err(|e| internal_error("PriceScheduleError", e))?;
    price_scheduler::update(
        &mut client,
        settings.enabled,
        settings.interval_minutes,
        settings.worker_concurrency as usize,
        &feeds,
    )
    .await
    .map_err(|e| internal_error("PriceScheduleError", e))?;
    if settings.enabled {
        price_scheduler::schedule_due(&mut client)
            .await
            .map_err(|e| internal_error("PriceScheduleError", e))?;
    }
    let status = pricing::status()
        .await
        .map_err(|e| internal_error("PriceStatusError", e))?;
    json_response(&status).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    post, path = "/api/prices/refresh", tag = "market",
    responses(
        (status = 202, description = "Manual refresh queued for selected feeds; current work and retry backoff are preserved", body = PricingStatus),
        (status = 400, description = "No selected feeds", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn refresh_prices() -> Result<impl IntoResponse, ErrorResponse> {
    let mut client = db_client()
        .await
        .map_err(|e| internal_error("PriceRefreshError", e))?;
    let config = price_scheduler::read(&client)
        .await
        .map_err(|e| internal_error("PriceRefreshError", e))?;
    if config.feeds.is_empty() {
        return Err(ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "NoPriceFeeds",
            "Select at least one price feed before requesting a refresh",
            None,
        ));
    }
    jobs::enqueue(&mut client, &config.feeds)
        .await
        .map_err(|e| internal_error("PriceRefreshError", e))?;
    let status = pricing::status()
        .await
        .map_err(|e| internal_error("PriceStatusError", e))?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[utoipa::path(
    post, path = "/api/prices/feeds/{feed}/refresh", tag = "market",
    params(("feed" = String, Path, description = "Supported feed key or CLI name")),
    responses(
        (status = 202, description = "Feed refresh queued, including when automatic scheduling is paused", body = PricingStatus),
        (status = 400, description = "Unsupported feed", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn refresh_price_feed(
    Path(feed): Path<String>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let feed = supported_feed(&feed)?;
    let mut client = db_client()
        .await
        .map_err(|e| internal_error("PriceRefreshError", e))?;
    jobs::enqueue(&mut client, &[feed])
        .await
        .map_err(|e| internal_error("PriceRefreshError", e))?;
    let status = pricing::status()
        .await
        .map_err(|e| internal_error("PriceStatusError", e))?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[utoipa::path(
    post, path = "/api/prices/feeds/{feed}/reset", tag = "market",
    params(("feed" = String, Path, description = "Supported feed key or CLI name")),
    responses(
        (status = 202, description = "Active job fenced and replaced with a fresh attempt; checkpoints, quotes, pins and history are preserved", body = PricingStatus),
        (status = 400, description = "Unsupported feed", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn reset_price_feed(
    Path(feed): Path<String>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let feed = supported_feed(&feed)?;
    let mut client = db_client()
        .await
        .map_err(|e| internal_error("PriceResetError", e))?;
    price_scheduler::reset(&mut client, feed)
        .await
        .map_err(|e| internal_error("PriceResetError", e))?;
    let status = pricing::status()
        .await
        .map_err(|e| internal_error("PriceStatusError", e))?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rust_decimal_macros::dec;

    #[test]
    fn manual_price_request_requires_an_exact_decimal_string() {
        let request: ManualPriceRequest =
            serde_json::from_str(r#"{"price":"123456789.12345678901234567890","as_of":null}"#)
                .unwrap();
        assert_eq!(request.price, dec!(123456789.12345678901234567890));
        assert_eq!(request.price.scale(), 20);
        assert!(request.as_of.is_none());
        assert!(serde_json::from_str::<ManualPriceRequest>(r#"{"price":123.45}"#).is_err());
        assert!(serde_json::from_str::<ManualPriceRequest>(r#"{"price":"invalid"}"#).is_err());
        assert!(serde_json::from_str::<ManualPriceRequest>(
            r#"{"price":"1","as_of":"not-a-date"}"#,
        )
        .is_err());
    }

    #[tokio::test]
    async fn instrument_mutations_reject_invalid_values_before_database_access() {
        for name in ["", " \t\n "] {
            let error = rename(
                Path("existing".into()),
                Ok(Json(RenameInstrumentRequest { name: name.into() })),
            )
            .await
            .err()
            .expect("Blank name must fail");
            assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
        }
        for price in [Decimal::ZERO, dec!(-0.01)] {
            let error = set_price(
                Path("existing".into()),
                Ok(Json(ManualPriceRequest { price, as_of: None })),
            )
            .await
            .err()
            .expect("Nonpositive price must fail");
            assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
        }
        let error = set_price(
            Path("existing".into()),
            Ok(Json(ManualPriceRequest {
                price: dec!(1),
                as_of: Some(Utc::now() + Duration::days(1)),
            })),
        )
        .await
        .err()
        .expect("Future timestamp must fail");
        assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
    }
}
