use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;

#[derive(Debug, Tabled, Serialize)]
pub struct Position {
    pub isin: String,
    pub units: Decimal,
}
