use axum::{extract::Query, http::StatusCode, response::IntoResponse};
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    database::{db_client, queries::composite::get_monthly_investment_activity},
    services::events::{get_events, EventType},
};

use super::{internal_error, json_response, parse_ymd_start};
use crate::api::errors::ErrorResponse;

#[derive(Debug, Deserialize, IntoParams)]
pub struct TimelineQuery {
    /// Inclusive start date (YYYY-MM-DD).
    pub start_date: String,
    pub isin: Option<String>,
    pub broker: Option<String>,
    pub event_type: Option<EventType>,
}

#[utoipa::path(
    get,
    path = "/api/timeline",
    tag = "portfolio",
    params(TimelineQuery),
    responses(
        (status = 200, description = "Unified event timeline", body = [crate::services::events::PortfolioEvent]),
        (status = 400, description = "Invalid date", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(("api_token" = []))
)]
pub async fn timeline(
    Query(query): Query<TimelineQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let year_start_timestamp = parse_ymd_start(&query.start_date)?;
    let end_date = Utc::now();

    let mut timeline = get_events(year_start_timestamp, end_date)
        .await
        .map_err(|e| {
            ErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "TimelineRetrievalError",
                &format!("Failed to load timeline: {e}"),
                None,
            )
        })?;

    if let Some(ref isin) = query.isin {
        timeline.retain(|event| event.identifier.as_deref() == Some(isin.as_str()));
    }
    if let Some(ref broker) = query.broker {
        timeline.retain(|event| event.broker == *broker);
    }
    if let Some(ref event_type) = query.event_type {
        timeline.retain(|event| event.event_type == *event_type);
    }

    timeline.sort_by_key(|event| std::cmp::Reverse(event.date));

    json_response(&timeline).map_err(|status| {
        ErrorResponse::new(
            status,
            "SerializationError",
            "Failed to serialize timeline",
            None,
        )
    })
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct InvestmentActivityQuery {
    /// Inclusive UTC start date (YYYY-MM-DD). Omit for all recorded activity.
    pub start_date: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/timeline/investment-activity",
    tag = "portfolio",
    params(InvestmentActivityQuery),
    responses(
        (status = 200, description = "Monthly UTC EUR net purchases (imported and manual buys minus sales, before fees), net cash added (cash-account deposits minus withdrawals), and dividends & interest (recorded gross EUR amounts before withholding tax, including imported/manual and linked/unlinked interest regardless of tax treatment). Separate measures, not additive; preserves archived history and excludes projected interest, valuations, opening/reconciliation balances, and loan principal movements.", body = [crate::database::models::investment_activity::MonthlyInvestmentActivity]),
        (status = 400, description = "Invalid start date", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn investment_activity(
    Query(query): Query<InvestmentActivityQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let start_date = query
        .start_date
        .as_deref()
        .map(parse_ymd_start)
        .transpose()?;
    let client = db_client()
        .await
        .map_err(|e| internal_error("InvestmentActivityRetrievalError", e))?;
    let activity = get_monthly_investment_activity(&client, start_date)
        .await
        .map_err(|e| internal_error("InvestmentActivityRetrievalError", e))?;
    json_response(&activity).map_err(|status| internal_error("SerializationError", status))
}
