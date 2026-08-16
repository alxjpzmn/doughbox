use axum::{extract::Query, response::IntoResponse};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::database::queries::{
    dividend::get_dividends, fx_conversion::get_fx_conversions, interest::get_interest,
    trade::get_trades, QueryFilter,
};
use crate::services::performance::get_buy_ins;

use super::{internal_error, json_response, parse_optional_ymd_end, parse_optional_ymd_start};
use crate::api::errors::ErrorResponse;

#[derive(Debug, Deserialize, IntoParams)]
pub struct TradeQuery {
    /// Current ISIN (listing changes are remapped).
    pub isin: Option<String>,
    /// Exact broker name as stored (see GET /api/brokers).
    pub broker: Option<String>,
    /// `Buy` or `Sell`.
    pub direction: Option<String>,
    /// Inclusive start date (YYYY-MM-DD).
    pub from_date: Option<String>,
    /// Inclusive end date (YYYY-MM-DD).
    pub until_date: Option<String>,
}

impl TradeQuery {
    fn into_filter(self) -> Result<QueryFilter, ErrorResponse> {
        Ok(QueryFilter {
            isin: self.isin,
            broker: self.broker,
            direction: self.direction,
            from_date: parse_optional_ymd_start(self.from_date.as_deref())?,
            until_date: parse_optional_ymd_end(self.until_date.as_deref())?,
            limit: None,
        })
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ResourceQuery {
    pub isin: Option<String>,
    pub broker: Option<String>,
    pub from_date: Option<String>,
    pub until_date: Option<String>,
}

impl ResourceQuery {
    fn into_filter(self) -> Result<QueryFilter, ErrorResponse> {
        Ok(QueryFilter {
            isin: self.isin,
            broker: self.broker,
            direction: None,
            from_date: parse_optional_ymd_start(self.from_date.as_deref())?,
            until_date: parse_optional_ymd_end(self.until_date.as_deref())?,
            limit: None,
        })
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct BuyInQuery {
    pub isin: Option<String>,
    pub broker: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/trades",
    tag = "trades",
    params(TradeQuery),
    responses(
        (status = 200, description = "Trades matching the filters", body = [crate::database::models::trade::Trade]),
        (status = 400, description = "Invalid date", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn trades(Query(query): Query<TradeQuery>) -> Result<impl IntoResponse, ErrorResponse> {
    let trades = get_trades(query.into_filter()?)
        .await
        .map_err(|e| internal_error("TradeRetrievalError", e))?;
    json_response(&trades).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/buy-ins",
    tag = "trades",
    params(BuyInQuery),
    responses(
        (status = 200, description = "WAC cost basis per instrument and broker", body = [crate::services::performance::BuyIn]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn buy_ins(Query(query): Query<BuyInQuery>) -> Result<impl IntoResponse, ErrorResponse> {
    let buy_ins = get_buy_ins(query.isin.as_deref(), query.broker.as_deref())
        .await
        .map_err(|e| internal_error("BuyInRetrievalError", e))?;
    json_response(&buy_ins).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/dividends",
    tag = "trades",
    params(ResourceQuery),
    responses(
        (status = 200, description = "Dividends matching the filters", body = [crate::database::models::dividend::Dividend]),
        (status = 400, description = "Invalid date", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn dividends(
    Query(query): Query<ResourceQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let dividends = get_dividends(query.into_filter()?)
        .await
        .map_err(|e| internal_error("DividendRetrievalError", e))?;
    json_response(&dividends).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/interest",
    tag = "trades",
    params(ResourceQuery),
    responses(
        (status = 200, description = "Interest payments matching the filters", body = [crate::database::models::interest::InterestPayment]),
        (status = 400, description = "Invalid date", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn interest(
    Query(query): Query<ResourceQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let interest = get_interest(query.into_filter()?)
        .await
        .map_err(|e| internal_error("InterestRetrievalError", e))?;
    json_response(&interest).map_err(|status| internal_error("SerializationError", status))
}

#[utoipa::path(
    get,
    path = "/api/fx-conversions",
    tag = "trades",
    params(ResourceQuery),
    responses(
        (status = 200, description = "FX conversions matching the filters", body = [crate::database::models::fx_conversion::FxConversion]),
        (status = 400, description = "Invalid date", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn fx_conversions(
    Query(query): Query<ResourceQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let conversions = get_fx_conversions(query.into_filter()?)
        .await
        .map_err(|e| internal_error("FxConversionRetrievalError", e))?;
    json_response(&conversions).map_err(|status| internal_error("SerializationError", status))
}
