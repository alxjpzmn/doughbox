use std::{panic::AssertUnwindSafe, time::Duration};

use chrono::Utc;
use futures::FutureExt;
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::{interval, MissedTickBehavior},
};
use tokio_postgres::Client;

use crate::database::{
    db_client,
    queries::{performance::add_performance_signal_to_db, telegram_schedule},
};
use super::{notifications, portfolio::get_portfolio_overview, portfolio_reports::{complete_snapshot, send_report}};

/// API-owned and independent of browser sessions, CLI calls, and price imports.
pub struct TelegramReportRuntime {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl TelegramReportRuntime {
    pub fn start() -> Self {
        let (stop, receiver) = watch::channel(false);
        Self { stop, task: tokio::spawn(scheduler(receiver)) }
    }

    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(30), &mut self.task).await.is_err() {
            // Abort owned workers and release their sessions. A subsequent API
            // process recovers running metadata without replaying this slot.
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
                if schedule_report(&mut workers).await.is_err() {
                    log::error!("Could not schedule or recover Telegram report; checking again next minute");
                }
            }
            result = workers.join_next(), if !workers.is_empty() => {
                if matches!(result, Some(Err(_))) {
                    log::error!("Telegram report worker stopped unexpectedly; status will recover as interrupted");
                }
            }
        }
    }
    // Pauses never cancel an in-flight report; shutdown first stops new claims.
    while workers.join_next().await.is_some() {}
}

async fn schedule_report(workers: &mut JoinSet<()>) -> anyhow::Result<()> {
    let db = db_client().await?;
    telegram_schedule::recover_interrupted(&db).await?;
    let schedule = telegram_schedule::read(&db).await?;
    if !schedule.enabled
        || schedule.next_report_at.is_none_or(|next| next > Utc::now())
        || !notifications::configured()
    {
        return Ok(());
    }
    if !telegram_schedule::try_lock(&db).await? {
        return Ok(());
    }
    if telegram_schedule::start_scheduled(&db, Utc::now()).await? {
        workers.spawn(run_report(db));
    } else {
        telegram_schedule::unlock(&db).await?;
    }
    Ok(())
}

async fn run_report(db: Client) {
    let report = AssertUnwindSafe(async {
        let overview = get_portfolio_overview().await
            .map_err(|_| "Portfolio report generation failed. Check database and market data availability.")?;
        let snapshot = complete_snapshot(&overview)
            .map_err(|_| "Portfolio valuation or returns are incomplete. No report was sent or snapshot saved.")?;
        send_report(&snapshot).await
            .map_err(|_| "Telegram delivery failed. Check server credentials, chat access, and upstream availability.")?;
        Ok::<_, &'static str>(snapshot)
    }).catch_unwind();
    tokio::pin!(report);
    let mut heartbeat = interval(Duration::from_secs(2));
    let result = loop {
        tokio::select! {
            result = &mut report => break result,
            _ = heartbeat.tick() => {
                // Losing the lock session cancels valuation/HTTP work rather
                // than letting another replica overlap an abandoned worker.
                if !matches!(tokio::time::timeout(Duration::from_secs(10), db.simple_query("SELECT 1")).await, Ok(Ok(_))) {
                    log::error!("Telegram report lock connection lost; status will recover as interrupted");
                    return;
                }
            }
        }
    };
    let (snapshot, error) = match result {
        Ok(Ok(snapshot)) => (Some(snapshot), None),
        Ok(Err(error)) => (None, Some(error)),
        Err(_) => (None, Some("Portfolio report interrupted unexpectedly before delivery could be confirmed.")),
    };
    if let Some(error) = error {
        log::warn!("{error}");
    }
    // Record actual delivery first. A later snapshot write failure must not turn
    // a confirmed Telegram send into a failure or cause the report to be resent.
    if telegram_schedule::finish(&db, error).await.is_err() {
        log::error!("Could not persist Telegram delivery outcome; status will recover as interrupted");
    }
    if let Some(snapshot) = snapshot {
        if add_performance_signal_to_db(snapshot).await.is_err() {
            log::error!("Telegram report was delivered but its performance snapshot could not be saved");
        }
    }
    if telegram_schedule::unlock(&db).await.is_err() {
        log::error!("Could not release Telegram report lock");
    }
}
