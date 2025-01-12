pub mod housekeeping;
pub mod import;
pub mod performance;
pub mod portfolio;
pub mod shared;
pub mod taxation;

use std::fs;

use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use housekeeping::housekeeping;
use import::import;
use performance::performance;
use portfolio::portfolio;
use shared::confirm_action;
use taxation::calculate_taxes;

use crate::{
    api,
    database::queries::{
        composite::{events_exist, EventFilter},
        fx_rate::get_most_recent_rate,
    },
    services::{market_data::fx_rates::fetch_historic_ecb_rates, parsers::extract_pdf_text},
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
    DebugPdf {
        path: String,
    },
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
            fetch_historic_ecb_rates(None).await?;
        }
    };

    match args {
        Command::Portfolio => {
            if events_exist(EventFilter::TradesOnly).await? {
                portfolio().await?;
            } else {
                println!(
                    "\x1b[31mPlease import trades first. Run with --help to learn more.\x1b[0m"
                );
            }
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
            if events_exist(EventFilter::TradesOnly).await? {
                performance().await?;
            } else {
                println!(
                    "\x1b[31mPlease import trades first. Run with --help to learn more.\x1b[0m"
                );
            }
        }
        Command::Housekeeping {} => {
            housekeeping().await?;
        }
        Command::Taxation {} => {
            if events_exist(EventFilter::All).await? {
                calculate_taxes().await?;
            } else {
                println!("\x1b[31mPlease import events (e.g. trades, dividends) first. Run with --help to learn how.\x1b[0m");
            }
        }
        Command::Api { silent: _ } => {
            println!("Starting web server...");
            api().await?;
        }
        Command::DebugPdf { path } => match fs::read(path.clone()) {
            Ok(buffer) => {
                let text = extract_pdf_text(&buffer)?;
                println!("{}", text);
            }
            Err(e) => {
                eprintln!("Failed to read {}: {:?}", path, e);
            }
        },
    }
    Ok(())
}
