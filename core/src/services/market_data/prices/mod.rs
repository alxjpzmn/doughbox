mod adapter;
mod cboe;
pub mod config;
pub mod deutsche_boerse;
mod euronext;
mod http;
pub mod types;
mod worker_limit;

#[cfg(test)]
mod tests;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use chrono::Utc;
use futures::{stream, StreamExt};
use reqwest::Client;
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{interval, MissedTickBehavior},
};

use crate::database::{
    db_client,
    queries::{background_job as jobs, position::get_positions, price, price_scheduler},
};
use crate::services::shared::logger;

use self::{
    config::{Feed, PriceConfig, MAX_WORKERS},
    types::{PriceJob, PricingStatus},
    worker_limit::WorkerLimit,
};

/// All workers share this bound through download and parsing, so bulk CSVs cannot
/// accumulate unbounded in-memory batches. Dashboard handlers never request providers.
const MAX_DOWNLOADS: usize = 2;

pub struct PriceRuntime {
    stop: watch::Sender<bool>,
    tasks: JoinSet<()>,
}

impl PriceRuntime {
    pub async fn start(mut config: PriceConfig, worker_only: bool) -> anyhow::Result<Self> {
        let mut db = db_client().await?;
        if worker_only {
            jobs::ensure_feeds(&db, &config.feeds).await?;
        } else {
            price_scheduler::initialize(&mut db).await?;
            config = price_scheduler::read(&db).await?;
        }
        let (stop, receiver) = watch::channel(false);
        let mut tasks = JoinSet::new();
        let client = http::http_client()?;
        let downloads = Arc::new(tokio::sync::Semaphore::new(MAX_DOWNLOADS));
        let limits = Arc::new(WorkerLimit::new(config.workers));
        let control_limits = limits.clone();
        let control_stop = receiver.clone();
        tasks.spawn(async move {
            control_loop(control_limits, control_stop).await;
        });
        if !worker_only {
            let receiver = receiver.clone();
            tasks.spawn(async move {
                scheduler(receiver).await;
            });
        }
        let feeds: Arc<[Feed]> = if worker_only {
            config.feeds.into()
        } else {
            Feed::ALL.into()
        };
        for _ in 0..MAX_WORKERS {
            let feeds = feeds.clone();
            let client = client.clone();
            let receiver = receiver.clone();
            let downloads = downloads.clone();
            let limits = limits.clone();
            tasks.spawn(async move {
                worker(feeds, client, downloads, limits, receiver).await;
            });
        }
        log::info!(
            "Price workers started; automatic schedule={}, interval={} minutes, worker concurrency={}",
            config.enabled && !worker_only,
            config.interval_minutes,
            config.workers
        );
        Ok(Self { stop, tasks })
    }

    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(result) = self.tasks.join_next().await {
                if let Err(error) = result {
                    log::error!("Price background task failed: {error}");
                }
            }
        })
        .await
        .is_err()
        {
            // Aborted jobs retain their leases and are safely reclaimed after expiry.
            self.tasks.shutdown().await;
        }
    }
}

pub async fn configuration() -> anyhow::Result<PriceConfig> {
    let mut db = db_client().await?;
    price_scheduler::initialize(&mut db).await?;
    price_scheduler::read(&db).await
}

async fn control_loop(limits: Arc<WorkerLimit>, mut stop: watch::Receiver<bool>) {
    let mut tick = interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut connection = None;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {
                let result = async {
                    let db = queue_connection(&mut connection).await?;
                    let (workers, log_level) = price_scheduler::controls(db).await?;
                    limits.set_limit(workers);
                    logger::apply_level(&log_level)
                }.await;
                if let Err(error) = result {
                    log::error!("Application control synchronization failed: {error:#}");
                    connection = None;
                }
            }
        }
    }
}

async fn scheduler(mut stop: watch::Receiver<bool>) {
    let mut tick = interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut connection = None;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {
                let result = async {
                    let db = queue_connection(&mut connection).await?;
                    price_scheduler::schedule_due(db).await
                }.await;
                if let Err(error) = result {
                    log::error!("Price scheduler failed: {error:#}");
                    connection = None;
                }
            }
        }
    }
}

async fn worker(
    feeds: Arc<[Feed]>,
    client: Client,
    downloads: Arc<tokio::sync::Semaphore>,
    limits: Arc<WorkerLimit>,
    mut stop: watch::Receiver<bool>,
) {
    let mut tick = interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut connection = None;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {}
        }
        if *stop.borrow() {
            break;
        }
        let result = async {
            let Some(_permit) = limits.try_acquire() else {
                return Ok(false);
            };
            let db = queue_connection(&mut connection).await?;
            if let Some(job) = jobs::claim(db, &feeds).await? {
                log::info!(
                    "Price job {} ({}) started; attempt {}",
                    job.id,
                    job.feed,
                    job.attempts
                );
                let result = tokio::select! {
                    biased;
                    result = run_job(&client, downloads.clone(), &job) => Some(result),
                    _ = stop.changed() => None,
                };
                let Some(result) = result else {
                    // The refresh future is dropped before releasing ownership, so it
                    // cannot commit more work after another worker claims the job.
                    jobs::release(db, &job).await?;
                    log::info!(
                        "Price job {} ({}) requeued on shutdown; committed batches retained",
                        job.id,
                        job.feed
                    );
                    return Ok(true);
                };
                match result {
                    Ok(()) => {
                        jobs::finish(db, &job).await?;
                        log::info!("Price job {} ({}) succeeded", job.id, job.feed);
                    }
                    Err(error) => {
                        log::error!("Price job {} ({}) failed: {error:#}", job.id, job.feed);
                        let retry = retry_delay(&job, &error);
                        jobs::fail(db, &job, &format!("{error:#}"), retry).await?;
                    }
                }
            }
            Ok::<_, anyhow::Error>(false)
        }
        .await;
        match result {
            Ok(true) => break,
            Ok(false) => {}
            Err(error) => {
                log::error!("Price worker failed: {error:#}");
                connection = None;
            }
        }
    }
}

async fn queue_connection(
    connection: &mut Option<tokio_postgres::Client>,
) -> anyhow::Result<&mut tokio_postgres::Client> {
    if connection.as_ref().is_none_or(|client| client.is_closed()) {
        let db = db_client().await?;
        db.batch_execute("SET statement_timeout = '60s'; SET lock_timeout = '10s'")
            .await?;
        *connection = Some(db);
    }
    connection
        .as_mut()
        .context("Price queue connection unavailable")
}

fn retry_delay(job: &PriceJob, error: &anyhow::Error) -> i32 {
    let base = match job.attempts {
        1 => 60,
        2 => 300,
        _ => 900,
    };
    let jitter = ((job.id.unsigned_abs() + job.lease_token.unsigned_abs()) % 15) as i32;
    let requested = error
        .downcast_ref::<http::RateLimited>()
        .map(|e| e.retry_after_seconds)
        .unwrap_or(0);
    (base + jitter).max(requested)
}

async fn run_job(
    client: &Client,
    downloads: Arc<tokio::sync::Semaphore>,
    job: &PriceJob,
) -> anyhow::Result<()> {
    let heartbeat_db = db_client().await?;
    heartbeat_db
        .batch_execute("SET statement_timeout = '60s'; SET lock_timeout = '10s'")
        .await?;
    let refresh = refresh_feed(client, downloads, Feed::from_key(&job.feed)?, Some(job));
    // Poll these concurrently: awaiting a heartbeat in a select branch can deadlock
    // against the refresh transaction's own job-row lock.
    tokio::select! {
        result = refresh => { result?; Ok(()) },
        result = keep_lease_alive(&heartbeat_db, job) => result,
    }
}

async fn keep_lease_alive(db: &tokio_postgres::Client, job: &PriceJob) -> anyhow::Result<()> {
    let mut heartbeat = interval(Duration::from_secs(30));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        heartbeat.tick().await;
        jobs::renew(db, job).await?;
    }
}

/// Returns counts for a dry run; without a job lease, this never writes prices or checkpoints.
async fn refresh_feed(
    client: &Client,
    downloads: Arc<tokio::sync::Semaphore>,
    feed: Feed,
    job: Option<&PriceJob>,
) -> anyhow::Result<(usize, usize)> {
    let started = Instant::now();
    log::info!("{}: starting price refresh", feed.key());
    let mut db = db_client().await?;
    db.batch_execute("SET statement_timeout = '60s'; SET lock_timeout = '10s'")
        .await?;
    let tracked = Arc::new(price::tracked_isins(&db).await?);
    if tracked.is_empty() {
        log::info!(
            "{}: no tracked equity ISINs; nothing to refresh",
            feed.key()
        );
        return Ok((0, 0));
    }
    let universe = price::universe_hash(&tracked);
    let checkpoints = if job.is_some() {
        price::checkpoints(&db, feed.key(), &universe).await?
    } else {
        Default::default()
    };
    log::info!(
        "{}: discovering last-48-hour batches for {} tracked ISINs",
        feed.key(),
        tracked.len()
    );
    let files = adapter::discover(client, feed).await?;
    let available_files = files.len();
    let now = Utc::now();
    let files: Vec<_> = files
        .into_iter()
        .filter(|file| {
            // Revalidate recent files for late edits; completed older minute batches are immutable.
            !checkpoints.contains_key(&file.key)
                || file.timestamp >= now - chrono::Duration::hours(1)
        })
        .collect();
    let total_files = files.len();
    log::info!(
        "{}: {available_files} batches in the last 48 hours; {total_files} to check, {} skipped by checkpoints",
        feed.key(),
        available_files - total_files
    );
    if job.is_some() {
        price::ensure_instruments(&db, &tracked).await?;
    }
    let mut pending = stream::iter(files.into_iter().map(|file| {
        let tracked = tracked.clone();
        let downloads = downloads.clone();
        let etag = checkpoints.get(&file.key).and_then(|c| c.etag.clone());
        let previous_hash = checkpoints.get(&file.key).map(|c| c.content_hash.clone());
        async move {
            let permit = downloads.acquire_owned().await?;
            log::debug!("{}: downloading {}", feed.key(), file.key);
            let download = adapter::download(client, feed, &file, etag.as_deref())
                .await
                .with_context(|| format!("Downloading {}", file.key))?;
            let Some(download) = download else {
                return Ok::<_, anyhow::Error>(None);
            };
            let hash = blake3::hash(&download.bytes).to_hex().to_string();
            if previous_hash.as_ref() == Some(&hash) {
                return Ok(None);
            }
            let (key, etag, reports) = tokio::task::spawn_blocking(move || {
                let reports = adapter::parse(feed, &file, &download.bytes, &tracked, Utc::now())
                    .with_context(|| format!("Parsing {}", file.key))?;
                Ok::<_, anyhow::Error>((file.key, download.etag, reports))
            })
            .await??;
            drop(permit);
            Ok(Some((key, hash, etag, reports)))
        }
    }))
    .buffered(MAX_DOWNLOADS);
    let (mut files_checked, mut files_processed, mut reports_processed) = (0, 0, 0);
    let mut last_progress = Instant::now();
    while let Some(result) = pending.next().await {
        if let Some((file_key, hash, etag, reports)) = result? {
            reports_processed += if let Some(job) = job {
                price::commit_file(
                    &mut db,
                    job,
                    &file_key,
                    &universe,
                    &hash,
                    etag.as_deref(),
                    reports,
                )
                .await?
            } else {
                reports.len()
            };
            files_processed += 1;
        }
        files_checked += 1;
        if files_checked == 1
            || files_checked == total_files
            || last_progress.elapsed() >= Duration::from_secs(30)
        {
            log::info!(
                "{}: checked {files_checked}/{total_files} batches ({:.0}%); {files_processed} changed, {} unchanged, {reports_processed} tracked reports; elapsed {}s",
                feed.key(),
                files_checked as f64 * 100.0 / total_files as f64,
                files_checked - files_processed,
                started.elapsed().as_secs()
            );
            last_progress = Instant::now();
        }
    }
    if let Some(job) = job {
        // Re-evaluate freshness tiers and split boundaries even if no new trade arrived.
        log::info!("{}: selecting current valuations", feed.key());
        price::reselect_all(&mut db, job, &tracked).await?;
    }
    log::info!(
        "{} refresh complete: {files_checked}/{total_files} batches checked, {files_processed} changed, {} unchanged, {reports_processed} tracked reports; elapsed {}s",
        feed.key(),
        files_checked - files_processed,
        started.elapsed().as_secs()
    );
    Ok((files_processed, reports_processed))
}

pub async fn refresh_now(
    config: PriceConfig,
    feeds: Vec<Feed>,
    dry_run: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !feeds.is_empty(),
        "Select at least one price feed or use --feed for a manual refresh"
    );
    let started = Instant::now();
    if dry_run {
        let client = http::http_client()?;
        let downloads = Arc::new(tokio::sync::Semaphore::new(MAX_DOWNLOADS));
        for feed in feeds {
            let (files, reports) = refresh_feed(&client, downloads.clone(), feed, None).await?;
            println!("{} dry run: {files} files, {reports} tracked reports; no prices or checkpoints written", feed.key());
        }
        return Ok(());
    }
    let mut db = db_client().await?;
    let ids = jobs::enqueue(&mut db, &feeds).await?;
    println!("Queued price jobs: {ids:?}. Refreshing batches from the last 48 hours.");
    let runtime = PriceRuntime::start(PriceConfig { feeds, ..config }, true).await?;
    // Bulk feeds can legitimately exceed thirty minutes on a cold, paced import.
    // Wait for terminal job states; Ctrl-C still releases work with checkpoints intact.
    let wait = async {
        let mut last_progress = None;
        loop {
            let states = jobs::job_states(&db, &ids).await?;
            if let Some((_, error)) = states.iter().find(|(status, _)| status == "failed") {
                bail!(
                    "Price refresh failed: {}",
                    error.as_deref().unwrap_or("unknown error")
                );
            }
            if last_progress.is_none_or(|last: Instant| last.elapsed() >= Duration::from_secs(30)) {
                let queued = states
                    .iter()
                    .filter(|(status, _)| status == "queued")
                    .count();
                let running = states
                    .iter()
                    .filter(|(status, _)| status == "running")
                    .count();
                let succeeded = states
                    .iter()
                    .filter(|(status, _)| status == "succeeded")
                    .count();
                println!(
                    "Price refresh: {queued} queued, {running} running, {succeeded}/{} complete; elapsed {}s",
                    ids.len(),
                    started.elapsed().as_secs()
                );
                last_progress = Some(Instant::now());
            }
            if states.iter().all(|(status, _)| status == "succeeded") {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };
    let result = tokio::select! {
        result = wait => result,
        _ = shutdown_signal() => Err(anyhow::anyhow!(
            "Price refresh interrupted; committed batches are retained"
        )),
    };
    runtime.shutdown().await;
    result?;
    println!("Price refresh complete in {}s", started.elapsed().as_secs());
    Ok(())
}

pub async fn status() -> anyhow::Result<PricingStatus> {
    let db = db_client().await?;
    let config = price_scheduler::read(&db).await?;
    let isins: Vec<_> = get_positions(None, None)
        .await?
        .into_iter()
        .map(|p| p.isin)
        .collect();
    Ok(PricingStatus {
        enabled: config.enabled,
        interval_minutes: config.interval_minutes,
        worker_concurrency: config.workers as u32,
        feeds: price::feed_status(&db).await?,
        quotes: price::quotes(&db, &isins).await?,
    })
}

pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
