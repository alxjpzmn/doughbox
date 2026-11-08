pub mod housekeeping;
pub mod import;
pub mod performance;
pub mod portfolio;
pub mod prices;
pub mod shared;
pub mod taxation;
pub mod users;

use std::fs;

use chrono::NaiveDate;
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use housekeeping::housekeeping;
use import::import;
use performance::performance;
use portfolio::portfolio;
use shared::confirm_action;
use taxation::{calculate_taxes, calculate_taxes_detailed};

use crate::{
    api,
    cli::portfolio::PortfolioArgs,
    database::queries::{
        composite::{events_exist, EventFilter},
        auth,
        fx_rate::get_most_recent_rate,
    },
    services::{market_data::fx_rates::fetch_historic_ecb_rates, parsers::extract_pdf_text},
    database::{db_client, models::auth::UserId, queries::application_settings, run_migrations},
    services::{auth::password::canonical_username, files::create_necessary_directories, shared::{env::check_for_env_variables, logger::apply_level}},
};

#[derive(Parser, Debug)]
struct Args {
    /// Canonical owner for direct-database financial operations.
    #[arg(long, global = true)]
    user: Option<String>,
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
    Portfolio {
        #[arg(short, long)]
        notify: bool,
    },
    Performance {},
    /// Fetch delayed instrument prices and inspect the durable refresh queue.
    Prices {
        #[command(subcommand)]
        command: prices::PricesCommand,
    },
    Taxation {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        with_events: bool,
    },
    DebugPdf {
        path: String,
    },
    Api {
        #[arg(short, long)]
        silent: bool,
    },
    /// Issue single-use setup/recovery links through trusted console access.
    Users {
        #[command(subcommand)]
        command: users::UsersCommand,
    },
}

pub async fn cli() -> anyhow::Result<()> {
    let args = Args::parse();
    let user_required = matches!(
        &args.cmd,
        Command::Import { .. } | Command::Housekeeping { .. } | Command::Portfolio { .. }
        | Command::Performance { .. } | Command::Taxation { .. }
        | Command::Prices { command: prices::PricesCommand::Status | prices::PricesCommand::Set { .. } | prices::PricesCommand::Unpin { .. } }
    );
    anyhow::ensure!(user_required || args.user.is_none(), "--user is not valid for this instance/public command");
    anyhow::ensure!(!user_required || args.user.is_some(), "This financial command requires --user <canonical-username>");
    let auth_service = if matches!(&args.cmd, Command::Api { .. } | Command::Users { .. }) {
        Some(crate::services::auth::AuthService::new(crate::services::auth::AuthConfig::from_env()?))
    } else { None };
    if !matches!(&args.cmd, Command::DebugPdf { .. }) {
        check_for_env_variables();
        run_migrations().await?;
        if !matches!(&args.cmd, Command::Users { .. }) {
            create_necessary_directories();
            let settings = application_settings::read(&db_client().await?).await?;
            apply_level(&settings.log_level)?;
        }
    }
    let user_id: Option<UserId> = if let Some(username) = args.user {
        let username = canonical_username(&username)?;
        let account = auth::by_username(&db_client().await?, &username).await?
            .ok_or_else(|| anyhow::anyhow!("No active, fully enrolled account has that username"))?;
        anyhow::ensure!(account.fully_enrolled(), "--user must select an active, fully enrolled account");
        Some(account.id)
    } else { None };
    let args = args.cmd;
    if user_required && !matches!(args, Command::Prices { .. }) {
        // fetch new fx rates when older than 4 days
        let four_days_ago = Utc::now().naive_utc().date() - Duration::days(4);
        let most_recent_fx_rate = get_most_recent_rate().await?;
        if most_recent_fx_rate < four_days_ago && confirm_action("fetch updated exchange rates") {
            for currency in crate::database::queries::composite::get_used_currencies(user_id.as_ref().expect("validated financial owner")).await? {
                fetch_historic_ecb_rates(Some(&currency)).await?;
            }
        }
    };

    match args {
        Command::Prices { command } => prices::run(user_id.as_ref(), command).await?,
        Command::Portfolio { notify } => {
            let user_id = user_id.as_ref().expect("validated financial owner");
            if events_exist(user_id, EventFilter::All).await? {
                let args = PortfolioArgs {
                    notify: Some(notify),
                };
                portfolio(user_id, args).await?;
            } else {
                println!(
                    "\x1b[31mPlease import trades first. Run with --help to learn more.\x1b[0m"
                );
            }
        }
        Command::Import { path, silent } => {
            let user_id = user_id.as_ref().expect("validated financial owner");
            import(user_id, &path).await?;

            if !silent {
                if confirm_action("run housekeeping (1/4)") {
                    housekeeping(user_id).await?;
                }
                if confirm_action("run portfolio calculation (2/4)") {
                    let args = PortfolioArgs { notify: None };
                    portfolio(user_id, args).await?;
                }
                if confirm_action("run performance calculation (3/4)") {
                    performance(user_id).await?;
                }
                if confirm_action("run tax calculation (4/4)") {
                    calculate_taxes(user_id, None, None).await?;
                }
            }
        }
        Command::Performance {} => {
            let user_id = user_id.as_ref().expect("validated financial owner");
            if events_exist(user_id, EventFilter::All).await? {
                performance(user_id).await?;
            } else {
                println!(
                    "\x1b[31mPlease import portfolio activity first. Run with --help to learn more.\x1b[0m"
                );
            }
        }
        Command::Housekeeping {} => {
            housekeeping(user_id.as_ref().expect("validated financial owner")).await?;
        }
        Command::Taxation {
            from,
            until,
            with_events,
        } => {
            let user_id = user_id.as_ref().expect("validated financial owner");
            if events_exist(user_id, EventFilter::All).await? {
                let from_date = from
                    .as_deref()
                    .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                    .transpose()?;
                let until_date = until
                    .as_deref()
                    .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                    .transpose()?;
                if with_events {
                    calculate_taxes_detailed(user_id, from_date, until_date).await?;
                } else {
                    calculate_taxes(user_id, from_date, until_date).await?;
                }
            } else {
                println!("\x1b[31mPlease import events (e.g. trades, dividends) first. Run with --help to learn how.\x1b[0m");
            }
        }
        Command::Api { silent: _ } => {
            println!("Starting web server...");
            api::api(auth_service.expect("validated API configuration")).await?;
        }
        Command::Users { command } => users::run(command, auth_service.expect("validated account configuration")).await?,
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
