use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use typeshare::typeshare;
use utoipa::ToSchema;

use crate::database::{
    models::{performance::PerformanceComponent, position::PositionWithValueAndAllocation},
    queries::{
        composite::{events_exist, EventFilter},
        instrument::batch_get_instrument_names,
        interest::unlinked_interest_total,
        position::get_positions,
        price::quotes,
    },
};

use super::{
    assets::{calculate_custom_holdings_as_of, summarize_custom_holdings},
    performance::{security_costs, SecurityHistoryGap},
    shared::util::round_to_decimals,
};

fn serialize_money<S: serde::Serializer>(amount: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
    Serialize::serialize(&round_to_decimals(*amount), serializer)
}

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioOverview {
    #[typeshare(serialized_as = "number")]
    pub generated_at: i64,
    // Keep exact aggregates internally for percentages and persisted snapshots.
    #[serde(serialize_with = "serialize_money")]
    pub total_value: Decimal,
    pub realized: Decimal,
    pub income: Decimal,
    #[serde(serialize_with = "serialize_money")]
    pub total_return_abs: Decimal,
    pub total_return_rel: Decimal,
    #[serde(serialize_with = "serialize_money")]
    pub total_invested: Decimal,
    pub returns_incomplete: bool,
    pub security_history_gaps: Vec<SecurityHistoryGap>,
    #[typeshare(serialized_as = "number")]
    pub unpriced_assets: i64,
    pub positions: Vec<PositionWithValueAndAllocation>,
    pub performance_components: Vec<PerformanceComponent>,
    pub valuation_complete: bool,
    pub unpriced_isins: Vec<String>,
}

pub async fn get_portfolio_overview() -> anyhow::Result<PortfolioOverview> {
    let as_of = Utc::now();
    let client = crate::database::db_client().await?;
    let security_costs = security_costs(as_of).await?;
    let security_history_gaps: Vec<_> = security_costs.iter()
        .flat_map(|cost| cost.history_gaps.iter().cloned()).collect();
    let realized: Decimal = security_costs.iter().map(|cost| cost.realized).sum();
    let dividends: Decimal = security_costs.iter().map(|cost| cost.income).sum();
    let unlinked_interest = unlinked_interest_total(&client, as_of).await?;
    let current_positions = get_positions(Some(as_of), None, None).await?;
    let custom_holdings = calculate_custom_holdings_as_of(as_of).await?;
    let custom_summary = summarize_custom_holdings(&custom_holdings);

    let mut total_position = dec!(0.0);

    let isins: Vec<_> = current_positions
        .iter()
        .map(|position| position.isin.clone())
        .collect();

    let price_quotes = quotes(&client, &isins).await?;
    let unpriced_isins: Vec<_> = price_quotes
        .iter()
        .filter(|quote| quote.price.is_none())
        .map(|quote| quote.isin.clone())
        .collect();
    let names = batch_get_instrument_names(&isins).await?;

    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    let mut positions_with_value: Vec<PositionWithValueAndAllocation> = current_positions
        .iter()
        .zip(&price_quotes)
        .map(|(position, quote)| {
            let value = quote.price.map(|price| price * position.units);
            PositionWithValueAndAllocation {
                asset_id: position.isin.clone(),
                isin: Some(position.isin.clone()),
                asset_class: "Security".to_string(),
                name: name_map
                    .get(&position.isin)
                    .unwrap_or(&&position.isin)
                    .to_string(),
                value,
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
                value: holding.current_value_eur,
                units: holding.units_or_balance,
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

    let security_values: HashMap<&str, Decimal> = positions_with_value.iter()
        .filter_map(|position| position.isin.as_deref().zip(position.value)).collect();
    let mut performance_components: Vec<_> = security_costs.iter().filter_map(|cost| {
        let value = if !cost.history_gaps.is_empty() {
            // Only recorded income and purchases are known for this holding;
            // its market value must not be treated as a gain on zero cost.
            dec!(0)
        } else {
            match security_values.get(cost.isin.as_str()) {
                Some(value) => *value,
                None if cost.units > dec!(0) => return None,
                None => dec!(0),
            }
        };
        Some(performance_component(
            cost.isin.clone(), value,
            value + cost.realized + cost.income - cost.remaining_cost,
            cost.invested_amount,
        ))
    }).collect();
    performance_components.extend(custom_holdings.iter().filter_map(|holding| {
        Some(performance_component(
            holding.asset_id.clone(), holding.current_value_eur?,
            holding.total_return_eur?, holding.invested_eur,
        ))
    }));
    if unlinked_interest != dec!(0) {
        performance_components.push(performance_component(
            "income:unlinked".into(), dec!(0), unlinked_interest, dec!(0),
        ));
    }
    for position in &mut positions_with_value {
        position.value = position.value.map(round_to_decimals);
        position.share = match (position.value, total_position > dec!(0)) {
            (Some(value), true) => Some(round_to_decimals(value / total_position * dec!(100))),
            _ => None,
        };
    }

    let total_return_abs: Decimal = performance_components.iter()
        .map(|component| component.total_value - component.total_invested).sum();
    let total_invested: Decimal = performance_components.iter()
        .map(|component| component.gross_invested).sum();
    let income = dividends + unlinked_interest
        + custom_holdings.iter().map(|holding| holding.income_eur).sum::<Decimal>();

    if current_positions.is_empty()
        && custom_holdings.is_empty()
        && !events_exist(EventFilter::All).await?
    {
        return Ok(PortfolioOverview {
            generated_at: Utc::now().timestamp(),
            total_value: dec!(0),
            total_return_rel: dec!(0),
            total_return_abs: dec!(0),
            total_invested: dec!(0),
            realized: dec!(0),
            income: dec!(0),
            returns_incomplete: false,
            security_history_gaps,
            unpriced_assets: 0,
            positions: vec![],
            performance_components: vec![],
            valuation_complete: true,
            unpriced_isins: vec![],
        });
    }

    Ok(PortfolioOverview {
        generated_at: Utc::now().timestamp(),
        total_value: total_position,
        total_return_rel: if total_invested != dec!(0) {
            round_to_decimals(total_return_abs / total_invested * dec!(100.0))
        } else {
            dec!(0)
        },
        total_return_abs,
        total_invested,
        income,
        returns_incomplete: custom_summary.incomplete_asset_count > 0 || !security_history_gaps.is_empty(),
        security_history_gaps,
        realized: round_to_decimals(realized + dividends + unlinked_interest + custom_holdings.iter()
            .map(|holding| holding.realized_return_eur).sum::<Decimal>()),
        unpriced_assets: custom_summary.incomplete_asset_count,
        positions: positions_with_value,
        performance_components,
        valuation_complete: unpriced_isins.is_empty(),
        unpriced_isins,
    })
}

fn performance_component(
    asset_id: String, value: Decimal, total_return: Decimal, gross_invested: Decimal,
) -> PerformanceComponent {
    PerformanceComponent {
        asset_id,
        total_value: value,
        total_invested: value - total_return,
        gross_invested,
    }
}
