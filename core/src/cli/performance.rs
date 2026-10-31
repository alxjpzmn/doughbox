use itertools::Itertools;
use owo_colors::{OwoColorize, Style};
use rust_decimal_macros::dec;
use spinners_rs::{Spinner, Spinners};

use tabled::{Table, Tabled};

use crate::{
    services::{performance::get_performance, shared::util::{format_currency, round_to_decimals}},
};

#[derive(Debug, Tabled)]
struct StringifiedPositionPerformance {
    isin: String,
    name: String,
    unrealized: String,
    realized: String,
    income: String,
    performance: String,
    #[tabled(rename = "nominal index proxy")]
    simulated: String,
    alpha: String,
    total_return: String,
}

pub async fn performance() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Calculating EUR P&L including recorded dividends...");
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
                income: format_currency(item.income, true),
                performance: format_currency(item.performance, true),
                simulated: item.simulated.map(|value| format_currency(value, true)).unwrap_or_else(|| "Unavailable".into()),
                alpha: item.alpha.map(|value| format_currency(value, true)).unwrap_or_else(|| "Unavailable".into()),
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
        "Economic EUR P&L includes fees and recorded gross dividends (signed corrections); \
         realized is trading-only. Return uses cumulative gross purchase spend. \
         Benchmark: nominal SP500 price-index proxy, not dividend reinvestment."
    );

    println!(
        "Total actual P&L {} vs. nominal index proxy P&L {}: alpha {}",
        format_currency(performance_overview.actual, true),
        performance_overview.simulated.map(|value| format_currency(value, true)).unwrap_or_else(|| "Unavailable".into()),
        performance_overview.alpha.map(|value| format_currency(value, true)).unwrap_or_else(|| "Unavailable".into())
    );

    sp.stop();
    Ok(())
}
