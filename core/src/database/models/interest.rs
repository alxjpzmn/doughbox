use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;
use typeshare::typeshare;
use utoipa::ToSchema;

use super::asset::{InterestOrigin, TaxTreatment};

#[typeshare]
#[derive(Debug, Clone, Tabled, Serialize, ToSchema)]
pub struct InterestPayment {
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: String,
    pub principal: String,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Decimal,
    pub withholding_tax_currency: String,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InterestRecord {
    pub id: String,
    pub asset_id: Option<String>,
    pub date: DateTime<Utc>,
    pub amount: Decimal,
    pub broker: Option<String>,
    pub principal: Option<String>,
    pub currency: String,
    pub amount_eur: Decimal,
    pub withholding_tax: Option<Decimal>,
    pub withholding_tax_currency: Option<String>,
    pub tax_treatment: TaxTreatment,
    pub origin: InterestOrigin,
}
