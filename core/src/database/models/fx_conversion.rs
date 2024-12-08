use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
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
