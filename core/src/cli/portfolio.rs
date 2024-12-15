use chrono::Utc;
use owo_colors::{OwoColorize, Style};
use serde::Serialize;
use spinners_rs::{Spinner, Spinners};
use tabled::{Table, Tabled};

use crate::{
    cli::shared::format_currency,
    database::{
        models::performance::PerformanceSignal, queries::performance::add_performance_signal_to_db,
    },
    services::portfolio::get_portfolio_overview,
};

#[derive(Debug, Tabled, Serialize, Clone)]
struct StringifiedPositionWithAllocation {
    isin: String,
    name: String,
    value: String,
    units: String,
    share: String,
}

pub async fn portfolio() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Getting portfolio positions...");
    sp.start();
    let position_overview = get_portfolio_overview().await?;

    let mut formatted_positions_with_allocation: Vec<StringifiedPositionWithAllocation> = vec![];
    for position in position_overview.positions {
        let item = StringifiedPositionWithAllocation {
            isin: position.isin.clone(),
            name: position.name,
            value: format_currency(position.value, true),
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
