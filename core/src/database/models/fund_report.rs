use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tabled::Tabled;
use tokio_postgres::Row;

#[derive(Debug, Tabled, Clone)]
pub struct FundTaxReport {
    pub id: i32,
    pub date: DateTime<Utc>,
    pub isin: String,
    pub currency: String,
    pub dividend: Decimal,
    pub dividend_aequivalent: Decimal,
    pub intermittent_dividends: Decimal,
    pub withheld_dividend: Decimal,
    pub wac_adjustment: Decimal,
}

impl FundTaxReport {
    pub fn from_row(row: &Row) -> anyhow::Result<FundTaxReport> {
        Ok(FundTaxReport {
            id: row.try_get("id")?,
            date: row.try_get("date")?,
            isin: row.try_get("isin")?,
            currency: row.try_get("currency")?,
            dividend: row.try_get("dividend")?,
            dividend_aequivalent: row.try_get("dividend_aequivalent")?,
            intermittent_dividends: row.try_get("intermittent_dividend")?,
            withheld_dividend: row.try_get("withheld_dividend")?,
            wac_adjustment: row.try_get("wac_adjustment")?,
        })
    }
}
