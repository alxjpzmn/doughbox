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
struct StringifiedMergedPositionPl {
    isin: String,
    name: String,
    unrealized_pl: String,
    realized_pl: String,
    pl: String,
    pl_simulated: String,
    real_vs_sim: String,
    return_on_equity: String,
}

pub async fn performance() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Calculating P&L for positions...");
    sp.start();

    let performance_overview = get_performance().await?;

    let negative_change_style = Style::new().red().bold();
    let positive_change_style = Style::new().green().bold();

    let merged_positions_with_cli_formatting: Vec<StringifiedMergedPositionPl> =
        performance_overview
            .position_pl
            .iter()
            .map(|item| StringifiedMergedPositionPl {
                isin: item.isin.clone(),
                name: item.name.clone(),
                unrealized_pl: format_currency(item.unrealized_pl, true),
                realized_pl: format_currency(item.realized_pl, true),
                pl: format_currency(item.pl, true),
                pl_simulated: format_currency(item.pl_simulated, true),
                real_vs_sim: format_currency(item.real_vs_sim, true),
                return_on_equity: format!(
                    "{}",
                    format!("{}%", round_to_decimals(item.return_on_equity)).style(
                        if item.return_on_equity > dec!(0.0) {
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
        format_currency(performance_overview.total_actual, true),
        format_currency(performance_overview.total_simulated_pl, true),
        format_currency(performance_overview.total_alpha, true)
    );

    sp.stop();
    Ok(())
}
