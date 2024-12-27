use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use typeshare::typeshare;

use crate::database::{
    models::position::{PositionWithValue, PositionWithValueAndAllocation},
    queries::{
        instrument::get_current_instrument_price,
        position::get_positions,
        trade::{get_realized_return, get_total_invested_value},
    },
};

use super::{instruments::names::get_current_instrument_name, shared::util::round_to_decimals};

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
    let mut positions_with_value: Vec<PositionWithValue> = vec![];

    for position in current_positions.iter() {
        let current_price = get_current_instrument_price(&position.isin).await?;
        let position_with_value = PositionWithValue {
            isin: position.isin.clone(),
            value: current_price * position.units,
            units: position.units,
        };
        positions_with_value.push(position_with_value);
        total_position += current_price * position.units;
    }

    positions_with_value.sort_by(|a, b| a.value.partial_cmp(&b.value).unwrap());

    let mut positions_with_allocation: Vec<PositionWithValueAndAllocation> = vec![];
    for position in positions_with_value {
        let position_share = position.value / total_position;
        let item = PositionWithValueAndAllocation {
            isin: position.isin.clone(),
            name: get_current_instrument_name(&position.isin).await?,
            value: round_to_decimals(position.value),
            units: round_to_decimals(position.units),
            share: round_to_decimals(position_share * dec!(100.0)),
        };
        positions_with_allocation.push(item);
    }
    let total_return_abs = round_to_decimals((total_position + realized) - invested);

    Ok(PortfolioOverview {
        generated_at: Utc::now().timestamp(),
        total_value: round_to_decimals(total_position),
        total_return_rel: round_to_decimals(total_return_abs / invested * dec!(100.0)),
        total_return_abs,
        realized: round_to_decimals(realized),
        positions: positions_with_allocation.clone(),
    })
}
