use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum T212ImportSource {
    Trading212Api,
    StatementCsv,
}

impl T212ImportSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trading212Api => "trading212_api",
            Self::StatementCsv => "statement_csv",
        }
    }
}

#[derive(Clone, Debug)]
pub struct T212ImportContext {
    pub source: T212ImportSource,
    pub account_id: Option<String>,
    pub sync_run_id: Option<String>,
    pub statement_job_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct T212ImportOutcome {
    pub batch_id: String,
    pub inserted_count: i32,
    pub matched_count: i32,
    pub review_count: i32,
    pub skipped_count: i32,
}

pub mod reviews;
mod activity;
mod runtime;
pub(crate) mod store;
pub use runtime::T212SyncRuntime;
#[cfg(test)]
pub(crate) use runtime::run_session as run_test_session;

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct T212SyncAccount {
    pub id: String,
    pub environment: String,
    pub broker_account_id: String,
    pub currency: String,
    pub schedule_enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub backfill_cursor: DateTime<Utc>,
    pub backfill_complete: bool,
    pub synced_through: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct T212SyncRun {
    pub id: String,
    pub account_id: String,
    pub trigger: String,
    pub status: String,
    pub phase: String,
    pub range_from: DateTime<Utc>,
    pub range_to: DateTime<Utc>,
    pub cursor: DateTime<Utc>,
    pub report_from: Option<DateTime<Utc>>,
    pub report_to: Option<DateTime<Utc>>,
    pub report_id: Option<String>,
    pub next_attempt_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub inserted_count: i32,
    pub matched_count: i32,
    pub review_count: i32,
    pub skipped_count: i32,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct T212ImportBatchSummary {
    pub id: String,
    pub account_id: Option<String>,
    pub sync_run_id: Option<String>,
    pub statement_job_id: Option<String>,
    pub source: String,
    pub status: String,
    pub report_from: Option<DateTime<Utc>>,
    pub report_to: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
    pub inserted_count: i32,
    pub matched_count: i32,
    pub review_count: i32,
    pub skipped_count: i32,
    pub skipped_actions: BTreeMap<String, i32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct T212SyncStatus {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub current_account_id: Option<String>,
    pub accounts: Vec<T212SyncAccount>,
    pub runs: Vec<T212SyncRun>,
    pub review_batches: Vec<T212ImportBatchSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct T212SyncRunResponse {
    pub run: T212SyncRun,
    pub status: T212SyncStatus,
}

#[derive(Debug)]
pub struct T212SyncError { pub code: &'static str, pub message: &'static str }
impl std::fmt::Display for T212SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.message) }
}
impl std::error::Error for T212SyncError {}

pub async fn status(user_id: &crate::database::models::auth::UserId) -> anyhow::Result<T212SyncStatus> {
    store::status(user_id, &crate::database::db_client().await?).await
}
pub async fn save_schedule(user_id: &crate::database::models::auth::UserId, enabled: bool) -> anyhow::Result<T212SyncStatus> {
    store::save_schedule(user_id, enabled).await
}
pub async fn request_run(user_id: &crate::database::models::auth::UserId, full_history: bool) -> anyhow::Result<T212SyncRunResponse> {
    store::request_run(user_id, full_history).await
}
