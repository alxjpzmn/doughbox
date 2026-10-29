use axum::{
    extract::{rejection::JsonRejection, Path},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::{
    api::errors::ErrorResponse,
    services::data_imports::{self, DataImportSchedulerSettings, DataImportsStatus},
};

use super::internal_error;

fn validate_source(source: &str) -> Result<(), ErrorResponse> {
    if !data_imports::supported(source) {
        return Err(ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "InvalidDataImportSource",
            "Supported import sources are polygon and ecb",
            None,
        ));
    }
    Ok(())
}

#[utoipa::path(
    get, path = "/api/data-imports", tag = "market",
    responses(
        (status = 200, description = "Persistent API/manual and scheduled import status, per-source schedules, availability, and stored row coverage; interrupted work is recovered as failed", body = DataImportsStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Could not read import status", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn get_data_imports() -> Result<impl IntoResponse, ErrorResponse> {
    let status = data_imports::status().await.map_err(|_| {
        internal_error("DataImportStatusError", "Could not read data import status")
    })?;
    Ok(Json(status))
}

#[utoipa::path(
    put, path = "/api/data-imports/{source}/scheduler", tag = "market",
    params(("source" = String, Path, description = "polygon (stock splits) or ecb (exchange rates)")),
    request_body = DataImportSchedulerSettings,
    responses(
        (status = 200, description = "Persisted per-source schedule and all source statuses. Enabling or changing an enabled cadence makes it due on the next API scheduler tick; identical saves preserve the deadline. Pausing does not cancel an active import.", body = DataImportsStatus),
        (status = 400, description = "Unsupported source or malformed JSON", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 415, description = "Request requires a JSON content type", body = ErrorResponse),
        (status = 422, description = "Invalid enabled flag or frequency; supported frequencies are daily, weekly, monthly", body = ErrorResponse),
        (status = 500, description = "Could not save import schedule", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn save_data_import_scheduler(
    Path(source): Path<String>,
    payload: Result<Json<DataImportSchedulerSettings>, JsonRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    validate_source(&source)?;
    let Json(settings) = payload.map_err(|rejection| {
        ErrorResponse::new(
            rejection.status(),
            "InvalidRequest",
            &rejection.body_text(),
            None,
        )
    })?;
    let status = data_imports::save_schedule(&source, settings)
        .await
        .map_err(|_| internal_error("DataImportScheduleError", "Could not save data import schedule"))?;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/data-imports/{source}/refresh", tag = "market",
    params(("source" = String, Path, description = "polygon (stock splits) or ecb (exchange rates)")),
    responses(
        (status = 202, description = "Import started asynchronously, or the existing active import was retained", body = DataImportsStatus),
        (status = 400, description = "Unsupported import source", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 503, description = "Polygon token is not configured", body = ErrorResponse),
        (status = 500, description = "Could not start import", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn refresh_data_import(
    Path(source): Path<String>,
) -> Result<impl IntoResponse, ErrorResponse> {
    validate_source(&source)?;
    if let Some(reason) = data_imports::unavailable_reason(&source) {
        return Err(ErrorResponse::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "DataImportUnavailable",
            reason,
            None,
        ));
    }
    let status = data_imports::refresh(&source)
        .await
        .map_err(|_| internal_error("DataImportRefreshError", "Could not start data import"))?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::{FromRequest, Request},
    };

    #[tokio::test]
    async fn invalid_schedule_requests_are_rejected_before_database_access() {
        for (body, expected) in [
            (r#"{"enabled":true,"frequency":"hourly"}"#, StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":"true","frequency":"daily"}"#, StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":true}"#, StatusCode::UNPROCESSABLE_ENTITY),
            (r#"{"enabled":true,"frequency":"daily""#, StatusCode::BAD_REQUEST),
        ] {
            let request = Request::builder()
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let payload = Json::<DataImportSchedulerSettings>::from_request(request, &()).await;
            let result = save_data_import_scheduler(Path("polygon".to_owned()), payload).await;
            assert_eq!(result.err().unwrap().into_response().status(), expected);
        }
        let result = save_data_import_scheduler(
            Path("unsupported".to_owned()),
            Ok(Json(DataImportSchedulerSettings {
                enabled: true,
                frequency: data_imports::DataImportFrequency::Daily,
            })),
        )
        .await;
        assert_eq!(
            result.err().unwrap().into_response().status(),
            StatusCode::BAD_REQUEST
        );
    }
}
