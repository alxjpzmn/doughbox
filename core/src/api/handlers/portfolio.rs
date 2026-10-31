use axum::{extract::Query, http::StatusCode, response::IntoResponse};
use chrono::{Datelike, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    database::queries::composite::{events_exist, EventFilter},
    services::{
        parsers::parse_timestamp, portfolio::get_portfolio_overview,
        positions::get_positions_overview, shared::env::is_running_in_docker,
    },
};

use super::{internal_error, json_response};
use crate::api::errors::{ErrorDetails, ErrorResponse};

#[utoipa::path(
    get,
    path = "/api/portfolio",
    tag = "portfolio",
    responses(
        (status = 200, description = "Current recorded allocations and economic return including gross EUR dividends and interest, before withholding. Missing quotes set valuation_complete=false and unpriced_isins. Incomplete security acquisition history is listed in security_history_gaps and sets returns_incomplete=true; affected instruments' trading P&L is excluded while recorded income remains included. Recorded holdings may also be incomplete. Custom assets lacking valuation or cost data increment unpriced_assets and set returns_incomplete. Partial returns are known amounts only: do not use their percentage or persist a snapshot. Linked custom income is counted once.", body = crate::services::portfolio::PortfolioOverview),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Empty portfolio", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn portfolio() -> Result<impl IntoResponse, ErrorResponse> {
    match get_portfolio_overview().await {
        Ok(portfolio_overview) => {
            if portfolio_overview.positions.is_empty() {
                let events_check_result =
                    events_exist(EventFilter::All).await.map_err(|e| {
                        internal_error(
                            "EventsExistError",
                            format!("Error while checking if events exist: {e}"),
                        )
                    })?;
                let error_details = ErrorDetails {
                    in_docker: Some(is_running_in_docker()),
                    events_present: Some(events_check_result),
                };

                if !events_check_result {
                    return Err(ErrorResponse::new(
                        StatusCode::NOT_FOUND,
                        "EmptyPortfolioError",
                        "Empty portfolio without events",
                        Some(error_details),
                    ));
                }
            }
            Ok(json_response(&portfolio_overview))
        }
        Err(_err) => {
            let events_check_result = events_exist(EventFilter::TradesOnly).await.map_err(|e| {
                internal_error(
                    "EventsExistError",
                    format!("Error while checking if events exist: {e}"),
                )
            })?;
            let error_details = ErrorDetails {
                in_docker: Some(is_running_in_docker()),
                events_present: Some(events_check_result),
            };
            Err(ErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PortfolioRetrievalError",
                "An error occurred while retrieving the portfolio overview.",
                Some(error_details),
            ))
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PositionsQuery {
    /// Holdings as of this date (YYYY-MM-DD). Defaults to today.
    pub date: Option<String>,
    /// Restrict to a single ISIN.
    pub isin: Option<String>,
    /// Exact broker name as stored (see GET /api/brokers).
    pub broker: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/positions",
    tag = "portfolio",
    params(PositionsQuery),
    responses(
        (status = 200, description = "Holdings at the given date", body = [crate::database::models::position::PositionWithName]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(("api_token" = []))
)]
pub async fn positions(
    Query(query): Query<PositionsQuery>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let date = query.date.unwrap_or_else(|| {
        let now = Utc::now();
        format!("{}-{:02}-{:02}", now.year(), now.month(), now.day())
    });
    let timestamp = parse_timestamp(format!("{date} 19:00:00").as_str())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let positions = get_positions_overview(
        Some(timestamp),
        query.isin.as_deref(),
        query.broker.as_deref(),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&positions)
}
