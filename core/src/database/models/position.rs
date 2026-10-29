use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;
use typeshare::typeshare;
use utoipa::ToSchema;

#[derive(Debug, Tabled, Serialize)]
pub struct Position {
    pub isin: String,
    pub units: Decimal,
}

#[typeshare]
#[derive(Debug, Tabled, Serialize, ToSchema)]
pub struct PositionWithName {
    pub isin: String,
    pub name: String,
    pub units: Decimal,
}

#[typeshare]
#[derive(Debug, Serialize, Clone, ToSchema)]
pub struct PositionWithValueAndAllocation {
    pub asset_id: String,
    pub isin: Option<String>,
    pub asset_class: String,
    pub name: String,
    pub value: Option<Decimal>,
    pub units: Decimal,
    pub unit_label: String,
    pub share: Option<Decimal>,
}
