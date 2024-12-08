pub mod housekeeping;
pub mod import;
pub mod performance;
pub mod portfolio;
pub mod shared;
pub mod taxation;

use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use housekeeping::housekeeping;
use import::import;
use performance::performance;
use portfolio::portfolio;
use shared::confirm_action;
use taxation::calculate_taxes;

use crate::{
    api, database::queries::fx_rate::get_most_recent_rate,
    services::market_data::fx_rates::fetch_historic_ecb_rates,
};

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

pub async fn cli() -> anyhow::Result<()> {
    let args = Args::parse();
    let args = args.cmd;
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
