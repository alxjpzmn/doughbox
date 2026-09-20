use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio_postgres::types::ToSql;

use crate::{
    database::{db_client, models::position::Position},
    services::instruments::{
        identifiers::get_changed_identifier, stock_splits::get_split_adjusted_units,
    },
};

use super::{listing_change::get_listing_changes, stock_split::get_stock_splits, QueryFilter};

pub async fn get_positions_for_isin(
    isin: &str,
    date: Option<DateTime<Utc>>,
) -> anyhow::Result<Decimal> {
    let date = if let Some(date) = date {
        date
    } else {
        Utc::now()
    };

    let positions = get_positions(Some(date), Some(isin), None).await?;

    let result = positions.first().unwrap().units;

    Ok(result)
}

pub async fn get_positions(
    date: Option<DateTime<Utc>>,
    isin: Option<&str>,
    broker: Option<&str>,
) -> anyhow::Result<Vec<Position>> {
    let client = db_client().await?;

    let date = date.unwrap_or_else(Utc::now);
    let filter = QueryFilter {
        isin: isin.map(str::to_owned),
        broker: broker.map(str::to_owned),
        until_date: Some(date),
        ..QueryFilter::default()
    };

    let mut query = String::from("select isin, direction, units, date, broker from trade where 1=1");
    let mut params: Vec<&(dyn ToSql + Sync)> = vec![];
    super::append_common_filters(&mut query, &mut params, &filter);

    let rows = client.query(&query, &params).await?;

    let mut stock_split_information = get_stock_splits().await?;
    let listing_changes = get_listing_changes().await?;

    let mut units_map: HashMap<String, Decimal> = HashMap::new();

    for row in rows {
        let trade_broker: String = row.get(4);
        if let Some(ref wanted) = filter.broker {
            if &trade_broker != wanted {
                continue;
            }
        }
        let isin = get_changed_identifier(&row.get::<usize, String>(0), listing_changes.clone());
        if let Some(ref wanted) = filter.isin {
            if &isin != wanted {
                continue;
            }
        }
        let direction: String = row.get(1);
        let units = row.get::<usize, Decimal>(2);
        let trade_date = row.get::<usize, DateTime<Utc>>(3);
        // insert into units map
        let entry = units_map
            .entry(isin.clone())
            .or_insert_with(|| Decimal::from(0));
        let split_adjusted_units =
            get_split_adjusted_units(&isin, units, trade_date, &mut stock_split_information);
        if direction == "Buy" {
            *entry += split_adjusted_units;
        } else if direction == "Sell" {
            *entry -= split_adjusted_units;
        }
    }

    let mut active_units: Vec<Position> = vec![];

    for (isin, units) in units_map {
        let position = Position { isin, units };

        if position.units > dec!(0) {
            active_units.push(position);
        }
    }

    Ok(active_units)
}
