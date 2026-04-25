use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TaxOptimization {
    pub date: DateTime<Utc>,
    pub broker: String,
    pub amount: Decimal,
    pub currency: String,
    pub amount_eur: Decimal,
    pub tax_type: String,  // "CapitalGains", "Dividend", "Interest"
    pub description: Option<String>,
    pub transaction_id: Option<String>,
}
