use chrono::Utc;
use owo_colors::{OwoColorize, Style};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use spinners_rs::{Spinner, Spinners};
use tabled::{Table, Tabled};

use crate::util::{
    db_helpers::{
        add_performance_signal_to_db, get_current_positions, get_total_invested_value,
        get_total_sell_value, PerformanceSignal,
    },
    general_helpers::format_currency,
    market_data_helpers::{get_current_equity_price, get_current_security_name},
    performance_helpers::round_to_decimals,
};

#[derive(Debug)]
struct EquityPositionWithValue {
    isin: String,
    current_value: Decimal,
    units: Decimal,
}

#[derive(Debug, Tabled, Serialize, Clone)]
pub struct EquityAllocationItem {
    isin: String,
    name: String,
    current_value: Decimal,
    units: Decimal,
    share: Decimal,
}

#[derive(Debug, Tabled, Serialize, Clone)]
struct FormattedEquityAllocationItem {
    isin: String,
    name: String,
    current_value: String,
    units: String,
    share: String,
}

#[derive(Debug, Serialize)]
pub struct PositionOverview {
    pub generated_at: i64,
    pub total_value: Decimal,
    pub total_roe_abs: Decimal,
    pub total_roe_rel: Decimal,
    pub positions: Vec<EquityAllocationItem>,
}

pub async fn get_position_overview() -> anyhow::Result<PositionOverview> {
    let total_sell_value = get_total_sell_value().await?;
    let total_invested_value = get_total_invested_value().await?;

    let current_positions = get_current_positions().await?;
    let mut total_position = dec!(0.0);
    let mut positions: Vec<EquityPositionWithValue> = vec![];
    for position in current_positions {
        let current_price = get_current_equity_price(&position.isin).await?;
        let position_and_value = EquityPositionWithValue {
            isin: position.isin,
            current_value: current_price * position.no_units,
            units: position.no_units,
        };
        positions.push(position_and_value);
        total_position += current_price * position.no_units;
    }

    positions.sort_by(|a, b| a.current_value.partial_cmp(&b.current_value).unwrap());

    let mut positions_with_allocation: Vec<EquityAllocationItem> = vec![];
    for position in positions {
        let position_share = position.current_value / total_position;
        let item = EquityAllocationItem {
            isin: position.isin.clone(),
            name: get_current_security_name(&position.isin).await?,
            current_value: round_to_decimals(position.current_value),
            units: round_to_decimals(position.units),
            share: round_to_decimals(position_share * dec!(100.0)),
        };
        positions_with_allocation.push(item);
    }

    Ok(PositionOverview {
        generated_at: Utc::now().timestamp(),
        total_value: round_to_decimals(total_position),
        total_roe_abs: round_to_decimals(
            (total_position + total_sell_value) - total_invested_value,
        ),
        total_roe_rel: round_to_decimals(
            (((total_position + total_sell_value) - total_invested_value) / total_invested_value)
                * dec!(100.0),
        ),
        positions: positions_with_allocation.clone(),
    })
}

pub async fn portfolio() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Getting portfolio positions...");
    sp.start();
    let position_overview = get_position_overview().await?;

    let mut formatted_positions_with_allocation: Vec<FormattedEquityAllocationItem> = vec![];
    for position in position_overview.positions {
        let item = FormattedEquityAllocationItem {
            isin: position.isin.clone(),
            name: position.name,
            current_value: format_currency(position.current_value, true),
            units: position.units.to_string(),
            share: format!("{:.2} %", position.share),
        };
        formatted_positions_with_allocation.push(item);
    }

    let performance_signal_to_save = PerformanceSignal {
        date: Utc::now(),
        total_value: position_overview.total_value,
        total_invested: position_overview.total_value - position_overview.total_roe_abs,
    };
    add_performance_signal_to_db(performance_signal_to_save).await?;

    let table = Table::new(&formatted_positions_with_allocation).to_string();

    sp.stop();
    println!("\n");
    println!("{}", table);
    println!("====");
    let total_position_cli_style = Style::new().black().on_white().bold();
    println!(
        "Current portfolio value: {}",
        format_currency(position_overview.total_value, true).style(total_position_cli_style)
    );
    println!(
        "Total gain: {}, {:.2}% ROE",
        format_currency(position_overview.total_roe_abs, true).style(total_position_cli_style),
        position_overview.total_roe_rel
    );
    Ok(())
}
