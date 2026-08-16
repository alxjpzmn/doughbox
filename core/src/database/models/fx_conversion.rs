use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use typeshare::typeshare;
use utoipa::ToSchema;

#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FxConversion {
    pub date: DateTime<Utc>,
    pub broker: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub from_currency: String,
    pub to_currency: String,
    pub date_added: DateTime<Utc>,
    pub fees: Decimal,
}
