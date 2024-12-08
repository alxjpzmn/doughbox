use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tabled::Tabled;
use tokio_postgres::Row;

#[derive(Debug, Tabled, Clone)]
pub struct Instrument {
    pub id: String,
    pub last_price_update: DateTime<Utc>,
    pub price: Decimal,
    pub name: String,
}
impl Instrument {
    pub fn from_row(row: &Row) -> Instrument {
        Instrument {
            id: row.try_get("id").unwrap(),
            last_price_update: row.try_get("last_price_update").unwrap(),
            price: row.try_get("price").unwrap(),
            name: row.try_get("name").unwrap(),
        }
    }
}
