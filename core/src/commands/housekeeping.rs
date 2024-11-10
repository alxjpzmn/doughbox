use itertools::Itertools;

use crate::util::{
    db_helpers::{
        add_stock_split_to_db, get_stock_splits, get_used_isins, seed_fx_conversion_db,
        seed_fx_rates_db, seed_ticker_conversion_db,
    },
    market_data_helpers::{
        fetch_historic_ecb_rates, get_stock_split_information, get_symbol_from_isin,
    },
    taxation_helpers::query_for_oekb_funds_data,
};

pub async fn housekeeping() -> anyhow::Result<()> {
    seed_fx_conversion_db().await?;
    seed_fx_rates_db().await?;
    seed_ticker_conversion_db().await?;
    let existing_splits = get_stock_splits().await?;
    let isins = get_used_isins().await?;
    for isin in isins {
        query_for_oekb_funds_data(&isin).await?;
        let symbol = get_symbol_from_isin(&isin, None).await?;
        println!("Symbol: {}", symbol);
        let split_events = get_stock_split_information(&symbol, &isin).await?;

        println!("Split events: {:?}", split_events);

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
