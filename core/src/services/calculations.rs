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

use crate::database::{
    db_client,
    queries::{calculation, performance::add_performance_signal_to_db},
};

use super::{
    performance::get_performance,
    portfolio::get_portfolio_overview,
    portfolio_reports::complete_snapshot,
    taxation::export_detailed_capital_gains_tax_report,
};

const SOURCES: [&str; 2] = ["performance", "taxation"];

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CalculationFrequency {
    Daily,
    Weekly,
    Monthly,
}

impl CalculationFrequency {
    pub fn key(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CalculationSchedulerSettings {
    pub enabled: bool,
    /// Daily/weekly intervals and calendar months are calculated in UTC.
    pub frequency: CalculationFrequency,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CalculationStatus {
    pub source: String,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Range of the latest claimed taxation calculation; null for performance
    /// and for an unbounded side of an all-history taxation calculation.
    pub from_date: Option<NaiveDate>,
    pub until_date: Option<NaiveDate>,
    pub schedule_enabled: bool,
    #[schema(value_type = CalculationFrequency)]
    pub schedule_frequency: String,
    pub next_refresh_at: Option<DateTime<Utc>>,
}

/// Read persisted metadata and recover interrupted runs, without calculating or
/// exporting anything. A live worker's advisory lock prevents false recovery.
pub async fn status(source: &str) -> anyhow::Result<CalculationStatus> {
    calculation::recover_interrupted(&mut db_client().await?, source).await
}

pub async fn save_schedule(
    source: &str,
    settings: CalculationSchedulerSettings,
) -> anyhow::Result<CalculationStatus> {
    calculation::update_schedule(
        &db_client().await?,
        source,
        settings.enabled,
        settings.frequency.key(),
    )
    .await
}

/// API-owned scheduler; CLI calculations never start this runtime.
pub struct CalculationRuntime {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl CalculationRuntime {
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
            // Aborting the loop drops its JoinSet and dedicated lock sessions.
            // A later scheduler tick or status read recovers interrupted work.
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
                        log::error!("Could not schedule or recover calculation for {source}; retrying next tick");
                    }
                }
            }
            result = workers.join_next(), if !workers.is_empty() => {
                if matches!(result, Some(Err(_))) {
                    log::error!("Scheduled calculation worker stopped unexpectedly; status will recover as interrupted");
                }
            }
        }
    }
    // Pausing a schedule never aborts active work. Shutdown stops new claims
    // first and gives the runtime's existing workers time to finish.
    while workers.join_next().await.is_some() {}
}

async fn schedule_source(source: &str, workers: &mut JoinSet<()>) -> anyhow::Result<()> {
    let mut db = db_client().await?;
    // Also recover interrupted jobs while paused, without doing financial work.
    let status = calculation::recover_interrupted(&mut db, source).await?;
    let now = Utc::now();
    if status.schedule_enabled
        && status.next_refresh_at.is_some_and(|next| next <= now)
        && calculation::try_claim_scheduled(&mut db, source, now).await?
    {
        // Scheduled taxation always covers all history; manual UI ranges never
        // leak into scheduled work. Performance uses the same guarded pipeline.
        workers.spawn(run_calculation(db, source.to_owned(), None, None));
    }
    Ok(())
}

/// Explicit manual requests share scheduled workers' dedicated session lock.
/// Claiming is committed before spawn, so API replicas coalesce without
/// changing the active calculation's range or the persisted schedule.
pub async fn refresh(
    source: &str,
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> anyhow::Result<CalculationStatus> {
    anyhow::ensure!(
        source == "taxation" || (from.is_none() && until.is_none()),
        "Only taxation calculations accept a date range"
    );
    anyhow::ensure!(
        !matches!((from, until), (Some(from), Some(until)) if from > until),
        "Calculation start date must not follow its end date"
    );
    let mut db = db_client().await?;
    let (claimed, status) = calculation::try_claim(
        &mut db,
        source,
        from.map(|date| date.date_naive()),
        until.map(|date| date.date_naive()),
    )
    .await?;
    if claimed {
        let source = source.to_owned();
        tokio::spawn(async move {
            run_calculation(db, source, from, until).await;
        });
    }
    Ok(status)
}

async fn calculate(
    source: &str,
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    match source {
        "performance" => {
            let overview = get_portfolio_overview().await?;
            // Validate all security/custom-asset valuations and cost data before
            // exporting results or saving an authoritative history snapshot.
            let snapshot = complete_snapshot(&overview)?;
            get_performance().await?;
            // complete_snapshot includes per-asset components and gross invested
            // amounts, keeping the new history point filterable by asset/class.
            add_performance_signal_to_db(snapshot).await?;
            Ok(())
        }
        "taxation" => export_detailed_capital_gains_tax_report(from, until).await,
        _ => unreachable!("validated before starting worker"),
    }
}

async fn run_calculation(
    db: Client,
    source: String,
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) {
    let calculation = AssertUnwindSafe(calculate(&source, from, until)).catch_unwind();
    tokio::pin!(calculation);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            biased;
            _ = heartbeat.tick() => {
                // Drop the computation future when its lock session is lost or
                // unresponsive. A later status read recovers the abandoned row;
                // this worker never reconnects or continues without its lock.
                if !matches!(tokio::time::timeout(Duration::from_secs(10), db.simple_query("SELECT 1")).await, Ok(Ok(_))) {
                    log::error!("Calculation lock connection lost for {source}");
                    return;
                }
            }
            result = &mut calculation => break result,
        }
    };
    // Only the typed, locally generated history gap is safe to expose. Unknown
    // provider errors can contain credentials or untrusted response bodies.
    let fallback = match source.as_str() {
        "performance" => "Performance refresh failed. Check portfolio valuations and cost data, market-data availability, and database access, then retry.",
        _ => "Taxation recalculation failed. Check transaction data, exchange rates, fund reports, and database access, then retry.",
    };
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.downcast_ref::<super::performance::SecurityHistoryGap>()
            .map(ToString::to_string).unwrap_or_else(|| fallback.to_owned())),
        Err(_) => Some(fallback.to_owned()),
    };
    if let Some(error) = error.as_deref() {
        log::warn!("{error}");
    }
    if calculation::finish(&db, &source, error.as_deref()).await.is_err() {
        log::error!("Could not persist calculation outcome for {source}; status will recover as interrupted");
    }
    if calculation::unlock(&db, &source).await.is_err() {
        log::error!("Could not release calculation lock for {source}");
    }
}
