use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;
use typeshare::typeshare;

#[derive(Debug, Tabled, Serialize)]
pub struct Position {
    pub isin: String,
    pub units: Decimal,
}

#[typeshare]
#[derive(Debug, Tabled, Serialize)]
pub struct PositionWithName {
    pub isin: String,
    pub name: String,
    pub units: Decimal,
}

#[derive(Debug)]
pub struct PositionWithValue {
    pub isin: String,
    pub units: Decimal,
    pub value: Decimal,
}

#[typeshare]
#[derive(Debug, Tabled, Serialize, Clone)]
pub struct PositionWithValueAndAllocation {
    pub isin: String,
    pub name: String,
    pub value: Decimal,
    pub units: Decimal,
    pub share: Decimal,
}
