use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ListingChange {
    pub id: String,
    pub ex_date: DateTime<Utc>,
    pub from_factor: Decimal,
    pub to_factor: Decimal,
    pub from_identifier: String,
    pub to_identifier: String,
}
