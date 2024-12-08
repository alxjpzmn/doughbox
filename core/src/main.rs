mod api;
mod cli;
mod database;
mod services;
mod util;

use api::api;
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use cli::performance::performance;
use cli::portfolio::portfolio;
use cli::shared::confirm_action;
use cli::taxation::calculate_taxes;
use cli::{housekeeping::housekeeping, import::import};
use services::env::check_for_env_variables;
use services::files::create_necessary_directories;
use services::market_data::fx_rates::{fetch_historic_ecb_rates, get_most_recent_rate};
use util::migrations::run_migrations;

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
    Performance {},
    Taxation {},
    Api {
        #[arg(short, long)]
        silent: bool,
    },
}

async fn run_doughbox() -> anyhow::Result<()> {
    let args = Args::parse();
    let args = args.cmd;
    check_for_env_variables();
    create_necessary_directories();
    run_migrations().await?;

    if (args != Command::Api { silent: true }) {
        // fetch new fx rates when older than 4 days
        let four_days_ago = Utc::now().naive_utc().date() - Duration::days(4);
        let most_recent_fx_rate = get_most_recent_rate().await?;
        if most_recent_fx_rate < four_days_ago && confirm_action("fetch updated exchange rates") {
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
                if confirm_action("run housekeeping (1/4)") {
                    housekeeping().await?;
                }
                if confirm_action("run portfolio calculation (2/4)") {
                    portfolio().await?;
                }
                if confirm_action("run performance calculation (3/4)") {
                    performance().await?;
                }
                if confirm_action("run tax calculation (4/4)") {
                    calculate_taxes().await?;
                }
            }
        }
        Command::Performance {} => {
            performance().await?;
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
    run_doughbox().await?;
    Ok(())
}
