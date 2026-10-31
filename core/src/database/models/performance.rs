use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use typeshare::typeshare;
use utoipa::ToSchema;


#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PerformanceComponent {
    /// Canonical ISIN, custom asset ID, or `income:unlinked` for unallocated broker income.
    pub asset_id: String,
    pub total_value: Decimal,
    /// Net baseline: current value minus economic P&L, including recorded income.
    pub total_invested: Decimal,
    pub gross_invested: Decimal,
}
#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PerformanceSignal {
    pub date: DateTime<Utc>,
    pub total_value: Decimal,
    pub total_invested: Decimal,
    /// Cumulative gross EUR purchase spend, including purchase fees; absent in legacy snapshots.
    pub gross_invested: Option<Decimal>,
    /// Holding-level income-inclusive totals; absent in older aggregate-only snapshots.
    pub components: Option<Vec<PerformanceComponent>>,
}
