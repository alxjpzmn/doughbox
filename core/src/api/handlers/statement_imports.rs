use axum::{
    extract::{multipart::{MultipartError, MultipartRejection}, Multipart},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use utoipa::ToSchema;

use crate::{
    api::errors::ErrorResponse,
    database::models::statement_import::{StatementImportFile, StatementImportsStatus},
    services::statement_imports,
};

use super::internal_error;

pub const MAX_BODY_BYTES: usize = 100 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
const MAX_FILES: usize = 100;

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct StatementUploadRequest {
    /// One to 100 PDF/CSV files, each no larger than 25 MiB. The entire
    /// multipart request, including boundaries and headers, is limited to 100 MiB.
    #[schema(value_type = Vec<String>, format = Binary)]
    files: Vec<Vec<u8>>,
}

fn invalid_upload(message: &str) -> ErrorResponse {
    ErrorResponse::new(StatusCode::BAD_REQUEST, "InvalidStatementUpload", message, None)
}

fn too_large(message: &str) -> ErrorResponse {
    ErrorResponse::new(StatusCode::PAYLOAD_TOO_LARGE, "StatementUploadTooLarge", message, None)
}

fn multipart_error(error: MultipartError) -> ErrorResponse {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        too_large("The complete upload request must not exceed 100 MiB")
    } else {
        invalid_upload("Could not read multipart upload; send PDF/CSV files in fields named files")
    }
}

fn filename(value: Option<&str>) -> Result<String, ErrorResponse> {
    // Some clients supply a path rather than a basename. Never persist or pass
    // that path to the parser, even though uploaded bytes never touch disk.
    let name = value.and_then(|name| name.rsplit(['/', '\\']).next())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid_upload("Every files field must include a filename"))?;
    if name.chars().count() > 255 || name.chars().any(char::is_control) {
        return Err(invalid_upload("Filenames must be at most 255 characters and contain no control characters"));
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case("pdf") || extension.eq_ignore_ascii_case("csv")) {
        return Err(invalid_upload("Only PDF and CSV files are supported"));
    }
    Ok(name.to_owned())
}

async fn read_upload(payload: Result<Multipart, MultipartRejection>) -> Result<Vec<StatementImportFile>, ErrorResponse> {
    let mut multipart = payload.map_err(|rejection| {
        ErrorResponse::new(
            rejection.status(), "InvalidStatementUpload",
            "Expected a multipart/form-data request containing files fields", None,
        )
    })?;
    let mut files = Vec::new();
    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        if field.name() != Some("files") {
            return Err(invalid_upload("Every multipart field must be named files"));
        }
        if files.len() == MAX_FILES {
            return Err(invalid_upload("Upload at most 100 files in one request"));
        }
        let filename = filename(field.file_name())?;
        let mut contents = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            if chunk.len() > MAX_FILE_BYTES - contents.len() {
                return Err(too_large("Each statement file must not exceed 25 MiB"));
            }
            contents.extend_from_slice(&chunk);
        }
        if contents.is_empty() {
            return Err(invalid_upload("Statement files must not be empty"));
        }
        files.push(StatementImportFile { filename, contents });
    }
    if files.is_empty() {
        return Err(invalid_upload("Select at least one PDF or CSV file"));
    }
    Ok(files)
}

#[utoipa::path(
    get, path = "/api/statement-imports", tag = "portfolio",
    responses(
        (status = 200, description = "All queued/running uploads and the 100 latest completed uploads, newest id first. Uploads continue without an open browser. Interrupted imports fail and may have partially imported records.", body = StatementImportsStatus),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Could not read statement import status", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn get_statement_imports() -> Result<impl IntoResponse, ErrorResponse> {
    let status = statement_imports::status().await.map_err(|_| {
        internal_error("StatementImportStatusError", "Could not read statement import status")
    })?;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/statement-imports", tag = "portfolio",
    request_body(content = StatementUploadRequest, content_type = "multipart/form-data",
        description = "Repeated files fields, 1–100 PDF/CSV files (case-insensitive extensions), up to 25 MiB each and 100 MiB total multipart body. Validation and durable enqueue are all-or-nothing. Imports run one at a time across API replicas. Import failures can leave partially written records; review them before retrying. Ambiguous transactions require the interactive CLI importer."),
    responses(
        (status = 202, description = "Entire batch durably queued; returns the accepted jobs in upload order", body = StatementImportsStatus),
        (status = 400, description = "Missing, empty, unsupported, too many, or malformed multipart files; no files queued", body = ErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 413, description = "A file exceeds 25 MiB or the multipart body exceeds 100 MiB; no files queued", body = ErrorResponse),
        (status = 500, description = "Could not queue the batch; no files queued", body = ErrorResponse)
    ),
    security(("api_token" = []))
)]
pub async fn upload_statement_imports(
    payload: Result<Multipart, MultipartRejection>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let files = read_upload(payload).await?;
    let accepted = statement_imports::enqueue(&files).await.map_err(|_| {
        internal_error("StatementImportQueueError", "Could not queue statement files; no files were accepted")
    })?;
    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::DefaultBodyLimit,
        http::Request,
        routing::post,
        Router,
    };
    use tower::ServiceExt;

    async fn validate(payload: Result<Multipart, MultipartRejection>) -> Result<Json<Vec<String>>, ErrorResponse> {
        Ok(Json(read_upload(payload).await?.into_iter().map(|file| file.filename).collect()))
    }

    fn part(name: &str, filename: Option<&str>, contents: &str) -> String {
        let filename = filename.map(|name| format!("; filename=\"{name}\"")).unwrap_or_default();
        format!("--upload\r\nContent-Disposition: form-data; name=\"{name}\"{filename}\r\n\r\n{contents}\r\n")
    }

    async fn request(body: String, limit: usize) -> axum::response::Response {
        Router::new().route("/", post(validate)).layer(DefaultBodyLimit::max(limit))
            .oneshot(Request::builder().method("POST").uri("/")
                .header("content-type", "multipart/form-data; boundary=upload")
                .body(Body::from(body)).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn statement_upload_validation_accepts_batches_and_normalizes_filenames() {
        let body = format!("{}{}--upload--\r\n", part("files", Some("report.CSV"), "one"), part("files", Some("/tmp/report.PdF"), "two"));
        let response = request(body, MAX_BODY_BYTES).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(serde_json::from_slice::<Vec<String>>(&body).unwrap(), ["report.CSV", "report.PdF"]);
        assert_eq!(filename(Some("C:\\fakepath\\report.csv")).ok().unwrap(), "report.csv");
    }

    #[tokio::test]
    async fn statement_upload_validation_rejects_invalid_batches_before_enqueue() {
        for invalid in [
            "--upload--\r\n".to_owned(),
            format!("{}--upload--\r\n", part("files", Some("empty.csv"), "")),
            format!("{}--upload--\r\n", part("files", Some("report.txt"), "bad")),
            format!("{}--upload--\r\n", part("other", Some("report.csv"), "bad")),
            format!("{}--upload--\r\n", part("files", None, "bad")),
            format!("{}{}--upload--\r\n", part("files", Some("good.csv"), "valid"), part("files", Some("bad.txt"), "bad")),
            format!("{}--upload--\r\n", part("files", Some("report.csv"), "x").repeat(MAX_FILES + 1)),
            "--upload\r\nmalformed".to_owned(),
        ] {
            let response = request(invalid, MAX_BODY_BYTES).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"], "InvalidStatementUpload");
        }
        assert!(filename(Some("bad\0.csv")).is_err());
        assert!(filename(Some(&format!("{}.csv", "a".repeat(256)))).is_err());
    }

    #[tokio::test]
    async fn statement_upload_validation_bounds_file_and_request_bytes() {
        let body = format!("{}--upload--\r\n", part("files", Some("big.csv"), &"x".repeat(MAX_FILE_BYTES + 1)));
        assert_eq!(request(body, MAX_BODY_BYTES).await.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // Exercise the same body-limit middleware with a smaller test bound so
        // the test does not allocate a 100 MiB batch just to cross the limit.
        let body = format!("{}--upload--\r\n", part("files", Some("report.csv"), &"x".repeat(1024)));
        let response = request(body, 512).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"], "StatementUploadTooLarge");
    }
}
