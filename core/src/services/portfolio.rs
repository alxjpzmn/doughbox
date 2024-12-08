use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use tabled::Tabled;

use crate::database::queries::{
    position::get_positions,
    trade::{get_total_invested_value, get_total_sell_value},
};

use super::{
    instruments::names::get_current_instrument_name,
    market_data::prices::get_current_instrument_price, shared::round_to_decimals,
};

#[derive(Debug)]
struct PositionWithValue {
    isin: String,
    current_value: Decimal,
    units: Decimal,
}

#[derive(Debug, Tabled, Serialize, Clone)]
pub struct EquityAllocationItem {
    pub isin: String,
    pub name: String,
    pub current_value: Decimal,
    pub units: Decimal,
    pub share: Decimal,
}

#[derive(Debug, Serialize)]
pub struct PortfolioOverview {
    pub generated_at: i64,
    pub total_value: Decimal,
    pub total_sell_value: Decimal,
    pub total_roe_abs: Decimal,
    pub total_roe_rel: Decimal,
    pub positions: Vec<EquityAllocationItem>,
}

pub async fn get_portfolio_overview() -> anyhow::Result<PortfolioOverview> {
    let total_sell_value = get_total_sell_value().await?;
    let total_invested_value = get_total_invested_value().await?;

    let current_positions = get_positions(None, None).await?;
    let mut total_position = dec!(0.0);
    let mut positions_with_value: Vec<PositionWithValue> = vec![];

    for position in current_positions.iter() {
        let current_price = get_current_instrument_price(&position.isin).await?;
        let position_with_value = PositionWithValue {
            isin: position.isin.clone(),
            current_value: current_price * position.units,
            units: position.units,
        };
        positions_with_value.push(position_with_value);
        total_position += current_price * position.units;
    }

    positions_with_value.sort_by(|a, b| a.current_value.partial_cmp(&b.current_value).unwrap());

    let mut positions_with_allocation: Vec<EquityAllocationItem> = vec![];
    for position in positions_with_value {
        let position_share = position.current_value / total_position;
        let item = EquityAllocationItem {
            isin: position.isin.clone(),
            name: get_current_instrument_name(&position.isin).await?,
            current_value: round_to_decimals(position.current_value),
            units: round_to_decimals(position.units),
            share: round_to_decimals(position_share * dec!(100.0)),
        };
        positions_with_allocation.push(item);
    }
    let total_roe_abs =
        round_to_decimals((total_position + total_sell_value) - total_invested_value);

    Ok(PortfolioOverview {
        generated_at: Utc::now().timestamp(),
        total_value: round_to_decimals(total_position),
        total_roe_abs,
        total_sell_value: round_to_decimals(total_sell_value),
        total_roe_rel: round_to_decimals(total_roe_abs / total_invested_value * dec!(100.0)),
        positions: positions_with_allocation.clone(),
    })
}
