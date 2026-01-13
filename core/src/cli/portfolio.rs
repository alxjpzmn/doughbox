use chrono::Utc;
use log::info;
use owo_colors::{OwoColorize, Style};
use serde::Serialize;
use spinners_rs::{Spinner, Spinners};
use tabled::{Table, Tabled};

use crate::{
    cli::shared::format_currency,
    database::{
        models::performance::PerformanceSignal,
        queries::performance::{add_performance_signal_to_db, get_latest_performance_signal},
    },
    services::{notifications::Notification, portfolio::get_portfolio_overview},
};

#[derive(Debug, Tabled, Serialize, Clone)]
struct StringifiedPositionWithAllocation {
    isin: String,
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
        total_invested: position_overview.total_value - position_overview.total_return_abs,
    };

    match args.notify {
        Some(notify) => {
            if notify {
                let latest_performance_signal = get_latest_performance_signal().await?;
                if let Some(latest_performance_signal) = latest_performance_signal {
                    let total_value_now = performance_signal_to_save.total_value;
                    let total_invested_now = performance_signal_to_save.total_invested;
                    let unrealized_gain = total_value_now - total_invested_now;
                    let total_value_previously = &latest_performance_signal.total_value;
                    let total_invested_previously = &latest_performance_signal.total_invested;

                    let capital_flow = total_invested_now - total_invested_previously;
                    let value_change = total_value_now - total_value_previously;
                    let performance_delta = value_change - capital_flow;
                    let date_string = latest_performance_signal.date.format("%Y/%m/%d %H:%M");

                    let summary_text = format!(
                        "<b>Portfolio Update</b>\n\n\
                         <b>Current Values</b>\n\
                         • Current Portfolio Value: {}\n\
                         • Total Invested: {}\n\
                         • Unrealized P&amp;L: {}\n\n\
                         <b>Changes (since {date_string})</b>\n\
                         • Value Change: {}\n\
                         • Capital Flow: {}\n\
                         • Capital Gain: {}",
                        format_currency(total_value_now, true),
                        format_currency(total_invested_now, true),
                        format_currency(unrealized_gain, true),
                        format_currency(value_change, true),
                        format_currency(capital_flow, true),
                        format_currency(performance_delta, true),
                    );
                    let notification = Notification {
                        content: summary_text,
                    };
                    Notification::send(&notification).await?;
                }
            } else {
                info!("Notifications not enabled.")
            }
        }
        None => {
            info!("Notifications not enabled.")
        }
    }

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
        format_currency(position_overview.total_return_abs, true).style(total_position_cli_style),
        position_overview.total_return_rel
    );
    Ok(())
}
