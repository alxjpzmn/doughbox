pub mod composite;
pub mod dividend;
pub mod fund_report;
pub mod fx_conversion;
pub mod fx_rate;
pub mod instrument;
pub mod interest;
pub mod listing_change;
pub mod performance;
pub mod position;
pub mod stock_split;
pub mod tax_optimization;
pub mod ticker_conversion;
pub mod trade;

use chrono::{DateTime, Utc};
use tokio_postgres::types::ToSql;

#[derive(Debug, Default, Clone)]
pub struct QueryFilter {
    pub isin: Option<String>,
    pub broker: Option<String>,
    pub direction: Option<String>,
    pub from_date: Option<DateTime<Utc>>,
    pub until_date: Option<DateTime<Utc>>,
    pub limit: Option<i32>,
}

fn append_common_filters<'a>(
    sql: &mut String,
    params: &mut Vec<&'a (dyn ToSql + Sync)>,
    filter: &'a QueryFilter,
) {
    if let Some(ref broker) = filter.broker {
        params.push(broker);
        sql.push_str(&format!(" AND broker = ${}", params.len()));
    }
    if let Some(ref from_date) = filter.from_date {
        params.push(from_date);
        sql.push_str(&format!(" AND date >= ${}", params.len()));
    }
    if let Some(ref until_date) = filter.until_date {
        params.push(until_date);
        sql.push_str(&format!(" AND date <= ${}", params.len()));
    }
    if let Some(ref direction) = filter.direction {
        params.push(direction);
        sql.push_str(&format!(" AND direction = ${}", params.len()));
    }
}
