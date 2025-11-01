use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;

#[derive(Debug, Tabled, Clone, Serialize)]
pub struct Trade {
    pub broker: String,
    pub date: DateTime<Utc>,
    pub units: Decimal,
    pub avg_price_per_unit: Decimal,
    pub eur_avg_price_per_unit: Decimal,
    pub security_type: String,
    pub direction: String,
    pub currency: String,
    pub isin: String,
    pub date_added: DateTime<Utc>,
    pub fees: Decimal,
    pub withholding_tax: Decimal,
    pub withholding_tax_currency: String,
}

#[allow(dead_code)]
pub struct TradeWithHash {
    pub broker: String,
    pub date: DateTime<Utc>,
    pub isin: String,
    pub avg_price_per_unit: Decimal,
    pub eur_avg_price_per_unit: Decimal,
    pub units: Decimal,
    pub direction: String,
    pub security_type: String,
    pub currency: String,
    pub date_added: DateTime<Utc>,
    pub fees: Decimal,
    pub withholding_tax: Decimal,
    pub withholding_tax_currency: String,
    pub hash: String,
}
