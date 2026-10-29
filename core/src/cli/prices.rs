use chrono::{DateTime, Utc};
use clap::Subcommand;
use rust_decimal::Decimal;

use crate::{
    database::{db_client, queries::price},
    services::market_data::prices::{
        self,
        config::{Feed, PriceConfig},
        PriceRuntime,
    },
};

#[derive(Debug, Subcommand, PartialEq)]
pub enum PricesCommand {
    /// Refresh batches from the last 48 hours and wait for completion, with progress output.
    Refresh {
        #[arg(long, value_enum)]
        feed: Option<Feed>,
        /// Validate last-48-hour batches without writing prices, checkpoints, or queue jobs.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show scheduler, worker, and current-holding quote status as JSON.
    Status,
    /// Run queue consumers without the HTTP server or automatic scheduler.
    Worker,
    /// Pin a manual EUR unit price. Automatic refreshes and imports will not overwrite it.
    Set {
        isin: String,
        #[arg(long)]
        price: Decimal,
        /// RFC 3339 timestamp, defaults to now.
        #[arg(long)]
        as_of: Option<DateTime<Utc>>,
    },
    /// Remove a manual pin and reselect a stored market price or execution fallback.
    Unpin { isin: String },
}

pub async fn run(command: PricesCommand) -> anyhow::Result<()> {
    let config = prices::configuration().await?;
    match command {
        PricesCommand::Refresh { feed, dry_run } => {
            let feeds = feed
                .map(|feed| vec![feed])
                .unwrap_or_else(|| config.feeds.clone());
            prices::refresh_now(config, feeds, dry_run).await?;
        }
        PricesCommand::Status => println!(
            "{}",
            serde_json::to_string_pretty(&prices::status().await?)?
        ),
        PricesCommand::Worker => {
            let runtime = PriceRuntime::start(
                PriceConfig {
                    feeds: Feed::ALL.to_vec(),
                    ..config
                },
                true,
            )
            .await?;
            prices::shutdown_signal().await;
            runtime.shutdown().await;
        }
        PricesCommand::Set {
            isin,
            price: value,
            as_of,
        } => {
            price::set_manual(
                &mut db_client().await?,
                &isin,
                value,
                as_of.unwrap_or_else(Utc::now),
            )
            .await?;
            println!("Pinned {isin} at {value} EUR per unit");
        }
        PricesCommand::Unpin { isin } => {
            price::unpin(&mut db_client().await?, &isin).await?;
            println!("Removed manual pin for {isin}");
        }
    }
    Ok(())
}
