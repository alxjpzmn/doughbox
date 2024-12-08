use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;

#[derive(Debug, Tabled, Serialize)]
pub struct InterestPayment {
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: String,
    pub principal: String,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Decimal,
    pub witholding_tax_currency: String,
}
