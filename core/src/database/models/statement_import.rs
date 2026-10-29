use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StatementImportStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatementImportJob {
    /// Decimal PostgreSQL bigserial identifier, represented without JavaScript precision loss.
    pub id: String,
    pub filename: String,
    pub size_bytes: i64,
    pub status: StatementImportStatus,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Failures can follow partial writes; review existing transactions before retrying.
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatementImportsStatus {
    pub jobs: Vec<StatementImportJob>,
}

/// Validated upload. Contents never become a filesystem path or an API response.
#[derive(Debug)]
pub struct StatementImportFile {
    pub filename: String,
    pub contents: Vec<u8>,
}
