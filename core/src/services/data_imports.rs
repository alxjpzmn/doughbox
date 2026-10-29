use std::{panic::AssertUnwindSafe, time::Duration};

use chrono::{DateTime, NaiveDate, Utc};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::{interval, MissedTickBehavior},
};
use tokio_postgres::Client;
use utoipa::ToSchema;

use crate::{
    database::{db_client, queries::data_import},
    services::{
        instruments::stock_splits::update_stock_splits,
        market_data::fx_rates::fetch_historic_ecb_rates, shared::env::get_env_variable,
    },
};

const SOURCES: [&str; 2] = ["polygon", "ecb"];

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DataImportFrequency {
    Daily,
    Weekly,
    Monthly,
}

impl DataImportFrequency {
    pub fn key(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DataImportSchedulerSettings {
    pub enabled: bool,
    /// Daily/weekly intervals and calendar months are calculated in UTC.
    pub frequency: DataImportFrequency,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataImportStatus {
    pub source: String,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    /// Last API/manual or scheduled import; excludes CLI and implicit FX fetches.
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub schedule_enabled: bool,
    #[schema(value_type = DataImportFrequency)]
    pub schedule_frequency: String,
    pub next_refresh_at: Option<DateTime<Utc>>,
    /// Stored rows, including rows written outside API/manual and scheduled imports.
    pub record_count: i64,
    pub latest_record_date: Option<NaiveDate>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataImportsStatus {
    pub sources: Vec<DataImportStatus>,
}

pub fn supported(source: &str) -> bool {
    SOURCES.contains(&source)
}

pub fn unavailable_reason(source: &str) -> Option<&'static str> {
    if source == "polygon"
        && get_env_variable("POLYGON_TOKEN").is_none_or(|token| token.trim().is_empty())
    {
        Some("POLYGON_TOKEN is not configured. Set it on the server to import stock splits.")
    } else {
        None
    }
}

pub async fn status() -> anyhow::Result<DataImportsStatus> {
    let db = db_client().await?;
    for source in SOURCES {
        data_import::recover_interrupted(&db, source).await?;
    }
    read_status(&db).await
}

async fn read_status(db: &Client) -> anyhow::Result<DataImportsStatus> {
    let rows = db.query(
        "SELECT s.*,
         CASE WHEN source = 'polygon' THEN (SELECT count(*) FROM stock_split)
              ELSE (SELECT count(*) FROM fx_rate) END AS record_count,
         CASE WHEN source = 'polygon' THEN (SELECT max((ex_date AT TIME ZONE 'UTC')::date) FROM stock_split)
              ELSE (SELECT max(date) FROM fx_rate) END AS latest_record_date
         FROM data_import_status s ORDER BY CASE source WHEN 'polygon' THEN 0 ELSE 1 END",
        &[],
    ).await?;
    let sources = rows
        .into_iter()
        .map(|row| {
            let source: String = row.get("source");
            let reason = unavailable_reason(&source);
            DataImportStatus {
                source,
                available: reason.is_none(),
                unavailable_reason: reason.map(str::to_owned),
                status: row.get("status"),
                started_at: row.get("started_at"),
                finished_at: row.get("finished_at"),
                last_success_at: row.get("last_success_at"),
                last_error: row.get("last_error"),
                schedule_enabled: row.get("schedule_enabled"),
                schedule_frequency: row.get("schedule_frequency"),
                next_refresh_at: row.get("next_refresh_at"),
                record_count: row.get("record_count"),
                latest_record_date: row.get("latest_record_date"),
            }
        })
        .collect();
    Ok(DataImportsStatus { sources })
}

pub async fn save_schedule(
    source: &str,
    settings: DataImportSchedulerSettings,
) -> anyhow::Result<DataImportsStatus> {
    let db = db_client().await?;
    data_import::update_schedule(&db, source, settings.enabled, settings.frequency.key()).await?;
    // Saving only changes persisted configuration. The owned API scheduler
    // claims newly due work on its next tick, even with no clients polling.
    status().await
}

/// API-owned loop, independent of the price scheduler and absent from CLI imports.
pub struct DataImportRuntime {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl DataImportRuntime {
    pub fn start() -> Self {
        let (stop, receiver) = watch::channel(false);
        let task = tokio::spawn(scheduler(receiver));
        Self { stop, task }
    }

    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(30), &mut self.task)
            .await
            .is_err()
        {
            // Dropping the loop's JoinSet aborts its remaining workers and
            // releases their lock sessions; the next tick/GET recovers status.
            self.task.abort();
            let _ = self.task.await;
        }
    }
}

async fn scheduler(mut stop: watch::Receiver<bool>) {
    let mut tick = interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut workers = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            _ = tick.tick() => {
                for source in SOURCES {
                    if schedule_source(source, &mut workers).await.is_err() {
                        log::error!("Could not schedule or recover data import for {source}; retrying next tick");
                    }
                }
            }
            result = workers.join_next(), if !workers.is_empty() => {
                if matches!(result, Some(Err(_))) {
                    log::error!("Scheduled data import worker stopped unexpectedly; status will recover as interrupted");
                }
            }
        }
    }
    // Stop claiming first, then allow owned imports to finish without cancelling
    // a live import merely because a user paused its schedule.
    while workers.join_next().await.is_some() {}
}

async fn schedule_source(source: &str, workers: &mut JoinSet<()>) -> anyhow::Result<()> {
    let db = db_client().await?;
    // Startup and every tick recover imports lost with a previous API process,
    // including paused/unavailable sources, without fetching provider data.
    data_import::recover_interrupted(&db, source).await?;
    let schedule = data_import::read_schedule(&db, source).await?;
    if !schedule.enabled
        || schedule.next_refresh_at.is_none_or(|next| next > Utc::now())
        || unavailable_reason(source).is_some()
    {
        return Ok(());
    }
    if !data_import::try_lock(&db, source).await? {
        return Ok(());
    }
    // This atomic recheck/advance follows the advisory lock. A competing manual
    // import coalesces via that same lock; pause/config saves serialize on the row.
    if data_import::start_scheduled(&db, source, Utc::now()).await? {
        workers.spawn(run_import(db, source.to_owned()));
    } else {
        data_import::unlock(&db, source).await?;
    }
    Ok(())
}

/// Acquire before spawning, so racing requests/processes cannot start a second
/// importer. The worker owns this dedicated session until its final state is saved.
pub async fn refresh(source: &str) -> anyhow::Result<DataImportsStatus> {
    anyhow::ensure!(supported(source), "Unsupported data import source");
    anyhow::ensure!(
        unavailable_reason(source).is_none(),
        "Data import is unavailable"
    );
    let db = db_client().await?;
    if data_import::try_lock(&db, source).await? {
        data_import::start(&db, source).await?;
        let source = source.to_owned();
        tokio::spawn(async move {
            run_import(db, source).await;
        });
    }
    status().await
}

async fn run_import(db: Client, source: String) {
    let import = AssertUnwindSafe(async {
        match source.as_str() {
            "polygon" => update_stock_splits().await,
            "ecb" => fetch_historic_ecb_rates(None).await,
            _ => unreachable!("validated before starting worker"),
        }
    })
    .catch_unwind();
    tokio::pin!(import);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    let result = loop {
        tokio::select! {
            result = &mut import => break result,
            _ = heartbeat.tick() => {
                // Stop fetching if the session holding the lock is lost. A later
                // GET or scheduler tick recovers the row once PostgreSQL releases it.
                if !matches!(tokio::time::timeout(Duration::from_secs(10), db.simple_query("SELECT 1")).await, Ok(Ok(_))) {
                    log::error!("Data import lock connection lost for {source}");
                    return;
                }
            }
        }
    };
    // Never persist/log upstream error strings: Polygon request URLs contain a
    // credential, and upstream bodies are untrusted. Keep a useful safe message.
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(_)) | Err(_) => Some(match source.as_str() {
            "polygon" => "Polygon stock-split import failed. Check server configuration and upstream availability, then retry.",
            _ => "ECB exchange-rate import failed. The upstream request, response, or database write failed; retry the import.",
        }),
    };
    if let Some(error) = error {
        log::warn!("{error}");
    }
    if data_import::finish(&db, &source, error).await.is_err() {
        log::error!("Could not persist data import outcome for {source}; status will recover as interrupted");
    }
    if data_import::unlock(&db, &source).await.is_err() {
        log::error!("Could not release data import lock for {source}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scheduler_settings_accept_only_boolean_flags_and_exact_frequencies() {
        for frequency in ["daily", "weekly", "monthly"] {
            let settings: DataImportSchedulerSettings = serde_json::from_value(json!({
                "enabled": true,
                "frequency": frequency,
            }))
            .unwrap();
            assert!(settings.enabled);
            assert_eq!(settings.frequency.key(), frequency);
            assert_eq!(serde_json::to_value(settings.frequency).unwrap(), frequency);
        }
        for invalid in [
            json!({"enabled": true, "frequency": "hourly"}),
            json!({"enabled": true, "frequency": "Daily"}),
            json!({"enabled": true, "frequency": ""}),
            json!({"enabled": true, "frequency": 1}),
            json!({"enabled": "true", "frequency": "daily"}),
            json!({"enabled": null, "frequency": "daily"}),
            json!({"enabled": true}),
            json!({"frequency": "daily"}),
            json!(null),
        ] {
            assert!(serde_json::from_value::<DataImportSchedulerSettings>(invalid).is_err());
        }
    }
}
