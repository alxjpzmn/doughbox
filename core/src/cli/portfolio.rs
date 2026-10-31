use log::info;
use owo_colors::{OwoColorize, Style};
use serde::Serialize;
use spinners_rs::{Spinner, Spinners};
use tabled::{Table, Tabled};

use crate::{
    services::{
        portfolio::get_portfolio_overview,
        portfolio_reports::{complete_snapshot, send_report},
        shared::util::format_currency,
    },
};

#[derive(Debug, Tabled, Serialize, Clone)]
struct StringifiedPositionWithAllocation {
    identifier: String,
    name: String,
    value: String,
    units: String,
    share: String,
}

pub struct PortfolioArgs {
    pub notify: Option<bool>,
}

pub async fn portfolio(args: PortfolioArgs) -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Getting portfolio positions...");
    sp.start();
    let position_overview = get_portfolio_overview().await?;
    let performance_signal_to_save = complete_snapshot(&position_overview)?;

    let mut formatted_positions_with_allocation: Vec<StringifiedPositionWithAllocation> = vec![];
    for position in position_overview.positions {
        let item = StringifiedPositionWithAllocation {
            identifier: position.isin.unwrap_or(position.asset_id),
            name: position.name,
            value: position
                .value
                .map(|value| format_currency(value, true))
                .unwrap_or_else(|| "Unpriced".to_string()),
            units: position.units.to_string(),
            share: position
                .share
                .map(|share| format!("{share:.2} %"))
                .unwrap_or_else(|| "-".to_string()),
        };
        formatted_positions_with_allocation.push(item);
    }


    if args.notify == Some(true) {
        send_report(&performance_signal_to_save).await?;
    } else {
        info!("Notifications not enabled.");
    }

    crate::database::queries::performance::add_performance_signal_to_db(performance_signal_to_save).await?;

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
        "Tracked P&L: {}, {:.2}% on gross contributions",
        format_currency(position_overview.total_return_abs, true).style(total_position_cli_style),
        position_overview.total_return_rel
    );
    Ok(())
}
