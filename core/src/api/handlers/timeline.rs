use axum::{extract::Query, http::StatusCode, response::IntoResponse};
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::services::events::{get_events, EventType};

use super::{json_response, parse_ymd_start};
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
