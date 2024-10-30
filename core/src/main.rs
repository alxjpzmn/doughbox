use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use commands::api::api;
use commands::pl::pl;
use commands::portfolio::portfolio;
use commands::taxation::calculate_taxes;
use commands::{housekeeping::housekeeping, import::import};
use util::db_helpers::{
    seed_instruments_db, seed_listing_changes_db, seed_performance_db, seed_stock_splits_db,
    seed_trades_db,
};
use util::general_helpers::{
    check_for_env_variables, confirm_action, create_necessary_directories,
};
use util::market_data_helpers::{fetch_historic_ecb_rates, get_most_recent_rate};

mod commands;
mod importers;
mod util;

#[derive(Parser, Debug)]
struct Args {
    #[clap(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand, PartialEq)]
enum Command {
    Import {
        path: String,
        #[arg(short, long)]
        silent: bool,
    },
    Housekeeping {},
    Portfolio,
    PL {},
    Taxation {},
    Api {
        #[arg(short, long)]
        silent: bool,
    },
}

async fn run_portfolio_tracker() -> anyhow::Result<()> {
    let args = Args::parse();
    let args = args.cmd;
    check_for_env_variables();
    create_necessary_directories();
    seed_listing_changes_db().await?;
    seed_stock_splits_db().await?;
    seed_trades_db().await?;
    seed_performance_db().await?;
    seed_instruments_db().await?;

    if (args != Command::Api { silent: true }) {
        // fetch new fx rates when older than 4 days
        let four_days_ago = Utc::now().naive_utc().date() - Duration::days(4);
        let most_recent_fx_rate = get_most_recent_rate().await?;
        if most_recent_fx_rate < four_days_ago && confirm_action("fetch updated exchange rates?") {
            fetch_historic_ecb_rates().await?;
        }
    };

    match args {
        Command::Portfolio => {
            portfolio().await?;
        }
        Command::Import { path, silent } => {
            import(&path).await?;

            if !silent {
                if confirm_action("run housekeeping (1/5)") {
                    housekeeping().await?;
                }
                if confirm_action("run portfolio calculation (3/5)") {
                    portfolio().await?;
                }
                if confirm_action("run P&L calculation (4/5)") {
                    pl().await?;
                }
                if confirm_action("run tax calculation (5/5)") {
                    calculate_taxes().await?;
                }
            }
        }
        Command::PL {} => {
            pl().await?;
        }
        Command::Housekeeping {} => {
            housekeeping().await?;
        }
        Command::Taxation {} => {
            calculate_taxes().await?;
        }
        Command::Api { silent: _ } => {
            println!("Starting web server...");
            api().await?;
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    run_portfolio_tracker().await?;
    Ok(())
}
