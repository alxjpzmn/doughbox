use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PerformanceSignal {
    pub date: DateTime<Utc>,
    pub total_value: Decimal,
    pub total_invested: Decimal,
}
