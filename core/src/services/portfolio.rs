use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use typeshare::typeshare;

use crate::database::{
    models::position::{PositionWithValue, PositionWithValueAndAllocation},
    queries::{
        composite::{events_exist, EventFilter},
        instrument::{batch_get_instrument_names, batch_get_instrument_prices},
        position::get_positions,
        trade::{get_realized_return, get_total_invested_value},
    },
};

use super::shared::util::round_to_decimals;

#[typeshare]
#[derive(Debug, Serialize)]
pub struct PortfolioOverview {
    #[typeshare(serialized_as = "number")]
    pub generated_at: i64,
    pub total_value: Decimal,
    pub realized: Decimal,
    pub total_return_abs: Decimal,
    pub total_return_rel: Decimal,
    pub positions: Vec<PositionWithValueAndAllocation>,
}

pub async fn get_portfolio_overview() -> anyhow::Result<PortfolioOverview> {
    let realized = get_realized_return().await?;
    let invested = get_total_invested_value().await?;

    let current_positions = get_positions(None, None).await?;

    let mut total_position = dec!(0.0);

    let isins: Vec<_> = current_positions
        .iter()
        .map(|position| position.isin.clone())
        .collect();

    let prices = batch_get_instrument_prices(&isins).await?;
    let names = batch_get_instrument_names(&isins).await?;

    let price_map: HashMap<_, _> = isins.iter().zip(prices.iter()).collect();
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    let mut positions_with_value: Vec<PositionWithValue> = current_positions
        .iter()
        .map(|position| {
            let binding = dec!(0.0);
            let current_price = *price_map.get(&position.isin).unwrap_or(&&binding);
            PositionWithValue {
                isin: position.isin.clone(),
                value: current_price * position.units,
                units: position.units,
            }
        })
        .collect();

    for position in &positions_with_value {
        total_position += position.value;
    }

    positions_with_value.sort_by(|a, b| a.value.partial_cmp(&b.value).unwrap());

    let positions_with_allocation: Vec<PositionWithValueAndAllocation> = positions_with_value
        .iter()
        .map(|position| {
            let position_share = position.value / total_position;
            PositionWithValueAndAllocation {
                isin: position.isin.clone(),
                name: name_map
                    .get(&position.isin)
                    .unwrap_or(&&position.isin.to_string())
                    .to_string(),
                value: round_to_decimals(position.value),
                units: round_to_decimals(position.units),
                share: round_to_decimals(position_share * dec!(100.0)),
            }
        })
        .collect();

    let total_return_abs = round_to_decimals((total_position + realized) - invested);

    if current_positions.is_empty() && !events_exist(EventFilter::TradesOnly).await? {
        return Ok(PortfolioOverview {
            generated_at: Utc::now().timestamp(),
            total_value: dec!(0),
            total_return_rel: dec!(0),
            total_return_abs: dec!(0),
            realized: dec!(0),
            positions: vec![],
        });
    }

    Ok(PortfolioOverview {
        generated_at: Utc::now().timestamp(),
        total_value: round_to_decimals(total_position),
        total_return_rel: round_to_decimals(total_return_abs / invested * dec!(100.0)),
        total_return_abs,
        realized: round_to_decimals(realized),
        positions: positions_with_allocation,
    })
}
