use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use typeshare::typeshare;
use utoipa::ToSchema;

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub key: String,
    pub timestamp: DateTime<Utc>,
    pub url: reqwest::Url,
}

pub struct Download {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PriceReport {
    pub id: String,
    pub isin: String,
    pub transaction_id: String,
    pub publisher: String,
    pub venue: String,
    pub executed_at: DateTime<Utc>,
    pub executed_ns: i64,
    pub published_ns: i64,
    // 0 = original, 1 = amendment, 2 = cancellation.
    pub revision: i32,
    pub revision_hash: String,
    pub price: Decimal,
    pub currency: String,
    pub eligible: bool,
}

#[derive(Debug, Clone)]
pub struct PriceJob {
    pub id: i64,
    pub feed: String,
    pub attempts: i32,
    pub lease_token: i64,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PriceQuote {
    pub isin: String,
    pub price: Option<Decimal>,
    pub currency: String,
    pub source: String,
    pub venue: Option<String>,
    pub as_of: Option<DateTime<Utc>>,
    pub received_at: Option<DateTime<Utc>>,
    pub pinned: bool,
    /// A clock-age indicator, not a trading-calendar or market-open assertion.
    pub older_than_hour: bool,
}

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceFeedStatus {
    pub feed: String,
    pub enabled: bool,
    pub scheduled: bool,
    pub next_refresh_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub job_status: Option<String>,
    pub attempts: i32,
    pub files_processed: i32,
    pub reports_processed: i32,
}

#[typeshare]
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct PriceSchedulerSettings {
    pub enabled: bool,
    pub interval_minutes: u32,
    #[schema(minimum = 1, maximum = 4)]
    pub worker_concurrency: u32,
    pub feeds: Vec<String>,
}

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PricingStatus {
    pub enabled: bool,
    pub interval_minutes: u32,
    pub worker_concurrency: u32,
    pub feeds: Vec<PriceFeedStatus>,
    pub quotes: Vec<PriceQuote>,
}
