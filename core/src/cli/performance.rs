use itertools::Itertools;
use owo_colors::{OwoColorize, Style};
use rust_decimal_macros::dec;
use spinners_rs::{Spinner, Spinners};

use tabled::{Table, Tabled};

use crate::{
    cli::shared::format_currency,
    services::{performance::get_performance, shared::util::round_to_decimals},
};

#[derive(Debug, Tabled)]
struct StringifiedPositionPerformance {
    isin: String,
    name: String,
    unrealized: String,
    realized: String,
    performance: String,
    simulated: String,
    alpha: String,
    total_return: String,
}

pub async fn performance() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Calculating P&L for positions...");
    sp.start();

    let performance_overview = get_performance().await?;

    let negative_change_style = Style::new().red().bold();
    let positive_change_style = Style::new().green().bold();

    let merged_positions_with_cli_formatting: Vec<StringifiedPositionPerformance> =
        performance_overview
            .position
            .iter()
            .map(|item| StringifiedPositionPerformance {
                isin: item.isin.clone(),
                name: item.name.clone(),
                unrealized: format_currency(item.unrealized, true),
                realized: format_currency(item.realized, true),
                performance: format_currency(item.performance, true),
                simulated: format_currency(item.simulated, true),
                alpha: format_currency(item.alpha, true),
                total_return: format!(
                    "{}",
                    format!("{}%", round_to_decimals(item.total_return)).style(
                        if item.total_return > dec!(0.0) {
                            positive_change_style
                        } else {
                            negative_change_style
                        }
                    )
                ),
            })
            .collect_vec();

    let table_merged_pl = Table::new(&merged_positions_with_cli_formatting).to_string();
    println!("{}", &table_merged_pl);

    println!(
        "Total actual PL {} vs. total simulated PL {}: {}",
        format_currency(performance_overview.actual, true),
        format_currency(performance_overview.simulated, true),
        format_currency(performance_overview.alpha, true)
    );

    sp.stop();
    Ok(())
}
