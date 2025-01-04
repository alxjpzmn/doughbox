use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Serialize)]
pub struct ErrorDetails {
    pub in_docker: Option<bool>,
    pub events_present: Option<bool>,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    status: u16,                   // HTTP status code
    error: String,                 // Short error identifier
    message: String,               // Human-readable error message
    details: Option<ErrorDetails>, // Optional details for debugging
}

impl ErrorResponse {
    pub fn new(
        status: StatusCode,
        error: &str,
        message: &str,
        details: Option<ErrorDetails>,
    ) -> Self {
        ErrorResponse {
            status: status.as_u16(),
            error: error.to_string(),
            message: message.to_string(),
            details,
        }
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }
}
