use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Tabled, Serialize)]
pub struct Dividend {
    pub isin: String,
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: String,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Decimal,
    pub withholding_tax_currency: String,
}
