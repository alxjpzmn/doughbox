use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use typeshare::typeshare;
use utoipa::ToSchema;

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PerformanceSignal {
    pub date: DateTime<Utc>,
    pub total_value: Decimal,
    pub total_invested: Decimal,
}
