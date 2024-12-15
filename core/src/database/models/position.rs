use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;

#[derive(Debug, Tabled, Serialize)]
pub struct Position {
    pub isin: String,
    pub units: Decimal,
}

#[derive(Debug)]
pub struct PositionWithValue {
    pub isin: String,
    pub units: Decimal,
    pub value: Decimal,
}

#[derive(Debug, Tabled, Serialize, Clone)]
pub struct PositionWithValueAndAllocation {
    pub isin: String,
    pub name: String,
    pub value: Decimal,
    pub units: Decimal,
    pub share: Decimal,
}
