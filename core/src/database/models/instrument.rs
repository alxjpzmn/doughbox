use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use tabled::Tabled;
use tokio_postgres::Row;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::services::market_data::prices::types::PriceQuote;

#[typeshare]
#[derive(Debug, Tabled, Clone, Serialize, ToSchema)]
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

#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InstrumentDetails {
    pub id: String,
    pub last_price_update: DateTime<Utc>,
    pub price: Decimal,
    pub name: String,
    /// Safe current valuation and its provenance; legacy price fields remain for compatibility.
    pub quote: PriceQuote,
}
