use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    models::position::PositionWithValueAndAllocation,
    queries::{
        composite::{events_exist, EventFilter},
        instrument::{batch_get_instrument_names, batch_get_instrument_prices},
        position::get_positions,
        trade::{get_realized_return, get_total_invested_value},
    },
};

use super::{
    assets::{calculate_custom_holdings, summarize_custom_holdings},
    shared::util::round_to_decimals,
};

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioOverview {
    #[typeshare(serialized_as = "number")]
    pub generated_at: i64,
    pub total_value: Decimal,
    pub realized: Decimal,
    pub total_return_abs: Decimal,
    pub total_return_rel: Decimal,
    pub returns_incomplete: bool,
    #[typeshare(serialized_as = "number")]
    pub unpriced_assets: i64,
    pub positions: Vec<PositionWithValueAndAllocation>,
}

pub async fn get_portfolio_overview() -> anyhow::Result<PortfolioOverview> {
    let realized = get_realized_return().await?;
    let invested = get_total_invested_value().await?;

    let current_positions = get_positions(None, None, None).await?;
    let custom_holdings = calculate_custom_holdings().await?;
    let custom_summary = summarize_custom_holdings(&custom_holdings);

    let mut total_position = dec!(0.0);

    let isins: Vec<_> = current_positions
        .iter()
        .map(|position| position.isin.clone())
        .collect();

    let prices = batch_get_instrument_prices(&isins).await?;
    let names = batch_get_instrument_names(&isins).await?;

    let price_map: HashMap<_, _> = isins.iter().zip(prices.iter()).collect();
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    let mut positions_with_value: Vec<PositionWithValueAndAllocation> = current_positions
        .iter()
        .map(|position| {
            let binding = dec!(0.0);
            let current_price = *price_map.get(&position.isin).unwrap_or(&&binding);
            let value = current_price * position.units;
            PositionWithValueAndAllocation {
                asset_id: position.isin.clone(),
                isin: Some(position.isin.clone()),
                asset_class: "Security".to_string(),
                name: name_map
                    .get(&position.isin)
                    .unwrap_or(&&position.isin.to_string())
                    .to_string(),
                value: Some(value),
                units: position.units,
                unit_label: "units".to_string(),
                share: None,
            }
        })
        .collect();

    positions_with_value.extend(
        custom_holdings
            .iter()
            .filter(|holding| holding.units_or_balance > dec!(0))
            .map(|holding| PositionWithValueAndAllocation {
                asset_id: holding.asset_id.clone(),
                isin: None,
                asset_class: holding.asset_class.to_string(),
                name: holding.name.clone(),
                value: holding.current_value_eur.map(round_to_decimals),
                units: round_to_decimals(holding.units_or_balance),
                unit_label: holding.unit_label.clone(),
                share: None,
            }),
    );

    for position in &positions_with_value {
        if let Some(value) = position.value {
            total_position += value;
        }
    }

    positions_with_value.sort_by(|a, b| {
        a.value
            .unwrap_or(dec!(-1))
            .partial_cmp(&b.value.unwrap_or(dec!(-1)))
            .unwrap()
    });

    for position in &mut positions_with_value {
        position.value = position.value.map(round_to_decimals);
        position.units = round_to_decimals(position.units);
        position.share = match (position.value, total_position > dec!(0)) {
            (Some(value), true) => Some(round_to_decimals(value / total_position * dec!(100))),
            _ => None,
        };
    }

    let total_return_abs = round_to_decimals(
        (total_position - custom_summary.current_value_eur + realized) - invested
            + custom_summary.total_return_eur,
    );
    let total_invested = invested
        + if custom_summary.incomplete_asset_count > 0 {
            custom_summary.known_invested_eur
        } else {
            custom_summary.invested_eur
        };

    if current_positions.is_empty()
        && custom_holdings.is_empty()
        && !events_exist(EventFilter::TradesOnly).await?
    {
        return Ok(PortfolioOverview {
            generated_at: Utc::now().timestamp(),
            total_value: dec!(0),
            total_return_rel: dec!(0),
            total_return_abs: dec!(0),
            realized: dec!(0),
            returns_incomplete: false,
            unpriced_assets: 0,
            positions: vec![],
        });
    }

    Ok(PortfolioOverview {
        generated_at: Utc::now().timestamp(),
        total_value: round_to_decimals(total_position),
        total_return_rel: if total_invested > dec!(0) {
            round_to_decimals(total_return_abs / total_invested * dec!(100.0))
        } else {
            dec!(0)
        },
        total_return_abs,
        returns_incomplete: custom_summary.incomplete_asset_count > 0,
        realized: round_to_decimals(realized),
        unpriced_assets: custom_summary.incomplete_asset_count,
        positions: positions_with_value,
    })
}
