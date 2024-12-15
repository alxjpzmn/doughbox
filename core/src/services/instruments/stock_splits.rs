use std::println;

use chrono::{DateTime, Utc};
use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::{
    database::queries::{
        composite::get_used_isins,
        stock_split::{add_stock_split_to_db, get_stock_splits},
    },
    services::market_data::{openfigi::get_symbol_from_isin, polygon::get_stock_split_information},
};

#[derive(Debug, Clone)]
pub struct StockSplit {
    pub id: String,
    pub ex_date: DateTime<Utc>,
    pub from_factor: Decimal,
    pub to_factor: Decimal,
    pub isin: String,
}

pub async fn update_stock_splits() -> anyhow::Result<()> {
    let existing_splits = get_stock_splits().await?;
    let isins = get_used_isins().await?;
    for isin in isins {
        let symbol = get_symbol_from_isin(&isin, None).await?;
        let split_events = get_stock_split_information(&symbol, &isin).await?;
        for split_event in split_events {
            let split_event_already_stored = !existing_splits
                .clone()
                .into_iter()
                .filter(|item| {
                    item.ex_date.date_naive() == split_event.ex_date.date_naive()
                        && item.isin == split_event.isin
                })
                .collect_vec()
                .is_empty();
            if split_event_already_stored {
                println!(
                    "Split event for {} on {} already stored, skipping.",
                    split_event.isin,
                    split_event.ex_date.date_naive()
                )
            } else {
                println!(
                    "Storing split event for {} on {}.",
                    split_event.isin,
                    split_event.ex_date.date_naive()
                );
                add_stock_split_to_db(split_event).await?;
            }
        }
    }
    Ok(())
}

pub fn get_split_adjusted_units(
    isin: &str,
    no_unadjusted_units: Decimal,
    date: DateTime<Utc>,
    split_information: &mut [StockSplit],
) -> Decimal {
    let relevant_splits = split_information
        .iter()
        .find(|item| isin == item.isin && date.timestamp() < item.ex_date.timestamp());
    if relevant_splits.is_none() {
        return no_unadjusted_units;
    }
    let mut no_adjusted_units = dec!(0.0);
    relevant_splits.into_iter().for_each(|relevant_split| {
        no_adjusted_units +=
            no_unadjusted_units * (relevant_split.to_factor / relevant_split.from_factor)
    });
    no_adjusted_units
}

pub fn get_split_adjusted_price_per_unit(
    isin: &str,
    unadjusted_price_per_unit: Decimal,
    date: DateTime<Utc>,
    split_information: &mut [StockSplit],
) -> Decimal {
    let relevant_splits = split_information
        .iter()
        .find(|item| isin == item.isin && date.timestamp() < item.ex_date.timestamp());
    if relevant_splits.is_none() {
        return unadjusted_price_per_unit;
    }
    let mut adjusted_price = dec!(0.0);
    relevant_splits.into_iter().for_each(|relevant_split| {
        adjusted_price +=
            unadjusted_price_per_unit / (relevant_split.to_factor / relevant_split.from_factor)
    });
    adjusted_price
}
