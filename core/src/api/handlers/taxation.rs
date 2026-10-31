use axum::{extract::{rejection::JsonRejection, Query}, http::StatusCode, response::IntoResponse, Json};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;
use utoipa::IntoParams;

use crate::{
    api::errors::ErrorResponse,
    services::{
        calculations::{self, CalculationSchedulerSettings, CalculationStatus},
        shared::constants::OUT_DIR,
        taxation::detailed_taxation_filename,
    },
};

use super::{internal_error, parse_optional_ymd_end, parse_optional_ymd_start};

#[derive(Debug, Deserialize, IntoParams)]
pub struct TaxationQuery {
    pub from_date: Option<String>,
    pub until_date: Option<String>,
}

impl TaxationQuery {
    fn date_range(&self) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>), ErrorResponse> {
        let from = parse_optional_ymd_start(self.from_date.as_deref())?;
        // Include every transaction on the selected final day, including subsecond events.
        let until = parse_optional_ymd_end(self.until_date.as_deref())?
            .map(|date| date + Duration::nanoseconds(999_999_999));
        if from.zip(until).is_some_and(|(from, until)| from > until) {
            return Err(ErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "InvalidDateRange",
                "The start date must not be after the end date.",
                None,
            ));
        }
        Ok((from, until))
    }
}

fn cached_range_matches(
    bundle: &Value,
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> bool {
    let Some(report) = bundle.get("report") else { return false };
    let matches = |field: &str, expected: Option<DateTime<Utc>>| {
        match (report.get(field), expected) {
            (Some(Value::Null), None) => true,
            (Some(Value::String(value)), Some(expected)) => DateTime::parse_from_rfc3339(value)
                .is_ok_and(|stored| stored.with_timezone(&Utc) == expected),
            _ => false,
        }
    };
    matches("from_date", from) && matches("until_date", until)
}

async fn cached_taxation(
    query: TaxationQuery,
    field: Option<&str>,
) -> Result<Json<Value>, ErrorResponse> {
    let (from, until) = query.date_range()?;
    let filename = detailed_taxation_filename(
        from.map(|date| date.date_naive()),
        until.map(|date| date.date_naive()),
    );
    let data = fs::read(format!("{OUT_DIR}/{filename}.json"))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ErrorResponse::new(
                    StatusCode::NOT_FOUND,
                    "TaxationCacheNotFound",
                    "No saved taxation report exists for this exact date range. Choose Recalculate taxation or run `doughbox taxation` with the same dates.",
                    None,
                )
            } else {
                internal_error("TaxationCacheReadError", "Could not read the saved taxation report.")
            }
        })?;
    let mut bundle: Value = serde_json::from_slice(&data).map_err(|_| {
        internal_error(
            "TaxationCacheInvalid",
            "The saved taxation report could not be read. Recalculate taxation to replace it.",
        )
    })?;
    // Older CLI versions wrote ranged reports to the all-history filename.
    // Never serve one of those as a complete report after the cache cutover.
    if !cached_range_matches(&bundle, from, until) {
        return Err(ErrorResponse::new(
            StatusCode::NOT_FOUND,
            "TaxationCacheNotFound",
            "The saved taxation report belongs to another date range. Recalculate taxation for the selected range.",
            None,
        ));
    }
    let value = match field {
        Some(field) => bundle.get_mut(field).map(Value::take).ok_or_else(|| {
            internal_error(
                "TaxationCacheInvalid",
                "The saved taxation report is incomplete. Recalculate taxation to replace it.",
            )
        })?,
        None => bundle,
    };
    Ok(Json(value))
}

#[utoipa::path(
    get,
    path = "/api/taxation",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 200, description = "Saved tax summary for the exact selected date range; never calculates", body = crate::services::taxation::TaxationReport),
        (status = 400, description = "Invalid dates or date range", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No saved report for this range; explicitly recalculate taxation first", body = ErrorResponse),
        (status = 500, description = "Could not read saved taxation report", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn taxation(
    Query(query): Query<TaxationQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    cached_taxation(query, Some("report")).await
}

#[utoipa::path(
    get,
    path = "/api/taxation/detailed",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 200, description = "Saved detailed tax bundle for the exact selected date range; never calculates"),
        (status = 400, description = "Invalid dates or date range", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No saved report for this range; explicitly recalculate taxation first", body = ErrorResponse),
        (status = 500, description = "Could not read saved taxation report", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn taxation_detailed(
    Query(query): Query<TaxationQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    cached_taxation(query, None).await
}

#[utoipa::path(
    get,
    path = "/api/taxation/transactions",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 200, description = "Saved per-transaction tax impacts for the exact selected date range; never calculates", body = [crate::services::taxation::TransactionTaxImpact]),
        (status = 400, description = "Invalid dates or date range", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No saved report for this range; explicitly recalculate taxation first", body = ErrorResponse),
        (status = 500, description = "Could not read saved taxation report", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn taxation_transactions(
    Query(query): Query<TaxationQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    cached_taxation(query, Some("transaction_impacts")).await
}

#[utoipa::path(
    get,
    path = "/api/taxation/status",
    tag = "taxation",
    responses(
        (status = 200, description = "Persistent manual and scheduled taxation calculation status and opt-in schedule; reading status never calculates", body = CalculationStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Could not read calculation status", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn get_taxation_status() -> Result<impl IntoResponse, ErrorResponse> {
    let status = calculations::status("taxation").await.map_err(|_| {
        internal_error("TaxationStatusError", "Could not read taxation calculation status.")
    })?;
    Ok(Json(status))
}

#[utoipa::path(
    post,
    path = "/api/taxation/refresh",
    tag = "taxation",
    params(TaxationQuery),
    responses(
        (status = 202, description = "Taxation calculation for the selected date range started asynchronously, or the active calculation was retained; does not change the full-history schedule", body = CalculationStatus),
        (status = 400, description = "Invalid dates or date range", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Could not start taxation calculation", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn refresh_taxation(
    Query(query): Query<TaxationQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let (from, until) = query.date_range()?;
    let status = calculations::refresh("taxation", from, until)
        .await
        .map_err(|_| {
            internal_error("TaxationRefreshError", "Could not start taxation calculation.")
        })?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[utoipa::path(
    put, path = "/api/taxation/scheduler", tag = "taxation",
    request_body = CalculationSchedulerSettings,
    responses(
        (status = 200, description = "Persisted opt-in taxation schedule (disabled by default, monthly cadence). Scheduled taxation always calculates full history, never a selected UI date range. Enabling or changing an enabled cadence makes it due on the next API scheduler tick, within one minute. Identical saves preserve the deadline. Pausing clears the deadline without cancelling active work.", body = CalculationStatus),
        (status = 400, description = "Malformed JSON", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 415, description = "Request requires a JSON content type", body = ErrorResponse),
        (status = 422, description = "Invalid enabled flag or frequency; supported frequencies are daily, weekly, monthly", body = ErrorResponse),
        (status = 500, description = "Could not save taxation schedule", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn save_taxation_scheduler(
    payload: Result<Json<CalculationSchedulerSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let Json(settings) = payload.map_err(|rejection| {
        ErrorResponse::new(
            rejection.status(),
            "InvalidRequest",
            &rejection.body_text(),
            None,
        )
    })?;
    let status = calculations::save_schedule("taxation", settings)
        .await
        .map_err(|_| internal_error("TaxationScheduleError", "Could not save taxation schedule"))?;
    Ok(Json(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Timelike};

    #[tokio::test]
    async fn invalid_taxation_schedules_are_rejected_before_database_access() {
        use axum::{body::Body, extract::{FromRequest, Request}};

        for (body, content_type, expected) in [
            (r#"{"enabled":true,"frequency":"hourly"}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":"true","frequency":"daily"}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":true}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"frequency":"monthly"}"#, Some("application/json"), StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":true,"frequency":"daily""#, Some("application/json"), StatusCode::BAD_REQUEST),
            (r#"{"enabled":true,"frequency":"daily"}"#, None, StatusCode::UNSUPPORTED_MEDIA_TYPE),
            (r#"{"enabled":false,"frequency":"weekly"}"#, Some("text/plain"), StatusCode::UNSUPPORTED_MEDIA_TYPE),
        ] {
            let mut request = Request::builder();
            if let Some(content_type) = content_type {
                request = request.header("content-type", content_type);
            }
            let request = request.body(Body::from(body)).unwrap();
            let payload = Json::<CalculationSchedulerSettings>::from_request(request, &()).await;
            let result = save_taxation_scheduler(payload).await;
            assert_eq!(result.err().unwrap().into_response().status(), expected);
        }
    }

    #[test]
    fn legacy_ranged_bundle_cannot_masquerade_as_full_history() {
        let query = TaxationQuery {
            from_date: Some("2025-01-01".into()), until_date: Some("2025-12-31".into()),
        };
        let (from, until) = query.date_range().ok().unwrap();
        let bundle = serde_json::json!({
            "report": { "from_date": from, "until_date": until }
        });
        assert!(!cached_range_matches(&bundle, None, None));
        assert!(cached_range_matches(&bundle, from, until));
        assert!(!cached_range_matches(&bundle, from, None));
        let full = serde_json::json!({ "report": { "from_date": null, "until_date": null } });
        assert!(cached_range_matches(&full, None, None));
        assert!(!cached_range_matches(&full, from, until));
    }

    #[test]
    fn selected_dates_include_the_complete_final_day() {
        let query = TaxationQuery {
            from_date: Some("2024-12-31".into()),
            until_date: Some("2024-12-31".into()),
        };
        let (from, until) = query.date_range().ok().unwrap();
        let day = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
        assert_eq!(from.unwrap().date_naive(), day);
        assert_eq!(from.unwrap().time().num_seconds_from_midnight(), 0);
        assert_eq!(until.unwrap().date_naive(), day);
        assert_eq!(until.unwrap().time().num_seconds_from_midnight(), 86_399);
        assert_eq!(until.unwrap().timestamp_subsec_nanos(), 999_999_999);
    }

    #[test]
    fn malformed_and_reversed_ranges_are_rejected() {
        for (from, until) in [
            ("2024-02-30", "2024-12-31"),
            ("2025-01-01", "2024-12-31"),
            ("2024-01-01", "not-a-date"),
        ] {
            let query = TaxationQuery {
                from_date: Some(from.into()),
                until_date: Some(until.into()),
            };
            let error = query.date_range().err().unwrap();
            assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
        }
    }
}
