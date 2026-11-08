mod adapter;
mod cboe;
pub mod config;
pub mod deutsche_boerse;
mod euronext;
pub(super) mod http;
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
use reqwest::Client;
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{interval, MissedTickBehavior},
};

use crate::database::{
    db_client,
    models::auth::UserId,
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
        let retention_stop = receiver.clone();
        tasks.spawn(async move {
            retention_loop(retention_stop).await;
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

async fn retention_loop(stop: watch::Receiver<bool>) {
    retention_loop_with_connection(stop, None).await;
}

async fn retention_loop_with_connection(
    mut stop: watch::Receiver<bool>,
    mut connection: Option<tokio_postgres::Client>,
) {
    let mut tick = interval(Duration::from_secs(60 * 60));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            _ = tick.tick() => {}
        }
        loop {
            let result = tokio::select! {
                biased;
                _ = stop.changed() => return,
                result = async {
                    let db = queue_connection(&mut connection).await?;
                    jobs::prune(db).await
                } => result,
            };
            match result {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    log::error!("Price retention cleanup failed: {error:#}");
                    connection = None;
                    break;
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

/// Returns portfolio-report admissions; every parsed batch also updates bulk latest trades.
async fn refresh_feed(
    client: &Client,
    downloads: Arc<tokio::sync::Semaphore>,
    feed: Feed,
    job: Option<&PriceJob>,
) -> anyhow::Result<(usize, usize)> {
    use std::collections::{HashSet, VecDeque};

    let started = Instant::now();
    let mut db = db_client().await?;
    db.batch_execute("SET statement_timeout = '60s'; SET lock_timeout = '10s'").await?;
    let (eligible, tracked) = if job.is_some() {
        let eligible = price::eligible_pairs_all_users(&db).await?;
        let tracked = eligible.iter().map(|(_, isin)| isin.clone()).collect();
        (eligible, tracked)
    } else {
        (Default::default(), price::tracked_isins_all_users(&db).await?)
    };
    let scope_hash = price::portfolio_scope_hash(&tracked);
    let checkpoints = if job.is_some() {
        price::checkpoints(&db, feed.key(), &scope_hash).await?
    } else {
        Default::default()
    };
    let mut dry_run = Some(price::DryRunAdmissions::default());
    let mut discovered = adapter::discover(client, feed).await?;
    discovered.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(a.key.cmp(&b.key)));
    let mut seen: HashSet<_> = discovered.iter().map(|file| file.key.clone()).collect();
    let mut available_files = discovered.len();
    let now = Utc::now();
    let mut queued: VecDeque<_> = discovered.into_iter().filter(|file| {
        !checkpoints.contains_key(&file.key)
            || file.timestamp >= now - chrono::Duration::hours(1)
    }).collect();
    let mut total_files = queued.len();
    log::info!(
        "{}: {available_files} available batches; {total_files} to check, newest first; bulk latest trades and portfolio reports",
        feed.key()
    );
    if job.is_some() {
        price::ensure_instruments(&db, &eligible).await?;
    }
    let mut pending = JoinSet::new();
    let mut last_discovery = Instant::now();
    let mut last_progress = Instant::now();
    let (mut files_checked, mut files_processed, mut reports_processed) = (0, 0, 0);
    while !queued.is_empty() || !pending.is_empty() {
        while pending.len() < MAX_DOWNLOADS {
            let Some(file) = queued.pop_front() else { break };
            let previous = checkpoints.get(&file.key);
            pending.spawn(download_file(
                client.clone(), downloads.clone(), feed, file,
                previous.and_then(|checkpoint| checkpoint.etag.clone()),
                previous.map(|checkpoint| checkpoint.content_hash.clone()),
            ));
        }
        let Some(result) = pending.join_next().await else { break };
        if let Some(file) = result.context("Price download task stopped")?? {
            let ingestion = match job {
                Some(job) => FileIngestion::Persist(job),
                None => FileIngestion::DryRun(&mut dry_run),
            };
            reports_processed += ingest_file(&mut db, feed, ingestion, file, &tracked, &eligible, &scope_hash).await?;
            files_processed += 1;
        }
        files_checked += 1;
        // Catch up the minute tail during a cold backfill. Rolling snapshots are
        // handled once per job; re-enqueuing their fixed keys would never finish.
        if feed != Feed::Euronext && last_discovery.elapsed() >= Duration::from_secs(60) {
            let mut incoming = adapter::discover(client, feed).await?;
            incoming.retain(|file| seen.insert(file.key.clone()));
            incoming.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(a.key.cmp(&b.key)));
            available_files += incoming.len();
            total_files += incoming.len();
            for file in incoming.into_iter().rev() {
                queued.push_front(file);
            }
            last_discovery = Instant::now();
        }
        if files_checked == 1 || files_checked == total_files || last_progress.elapsed() >= Duration::from_secs(30) {
            log::info!(
                "{}: checked {files_checked}/{total_files} batches; {files_processed} changed, {reports_processed} portfolio reports; elapsed {}s",
                feed.key(), started.elapsed().as_secs()
            );
            last_progress = Instant::now();
        }
    }
    if let Some(job) = job {
        // Always reselect: a new owner can join an unchanged ISIN union even
        // when every upstream file was skipped by the shared checkpoint.
        price::reselect_all(&mut db, job, &eligible).await?;
    }
    log::info!(
        "{} refresh complete: {files_checked}/{total_files} batches checked ({available_files} available); {reports_processed} portfolio reports; elapsed {}s",
        feed.key(), started.elapsed().as_secs()
    );
    Ok((files_processed, reports_processed))
}

struct DownloadedFile {
    file: types::SourceFile,
    bytes: Vec<u8>,
    etag: Option<String>,
    content_hash: String,
    permit: tokio::sync::OwnedSemaphorePermit,
}

async fn download_file(
    client: Client,
    downloads: Arc<tokio::sync::Semaphore>,
    feed: Feed,
    file: types::SourceFile,
    etag: Option<String>,
    previous_hash: Option<String>,
) -> anyhow::Result<Option<DownloadedFile>> {
    let permit = downloads.acquire_owned().await?;
    let download = adapter::download(&client, feed, &file, etag.as_deref()).await
        .with_context(|| format!("Downloading {}", file.key))?;
    let Some(download) = download else { return Ok(None) };
    let content_hash = blake3::hash(&download.bytes).to_hex().to_string();
    if previous_hash.as_ref() == Some(&content_hash) {
        return Ok(None);
    }
    Ok(Some(DownloadedFile {
        file, bytes: download.bytes, etag: download.etag, content_hash, permit,
    }))
}

enum FileIngestion<'a> {
    Persist(&'a PriceJob),
    // Taken while ingesting a file and restored only on success. An aborted file
    // invalidates the refresh's state instead of exposing partial admissions.
    DryRun(&'a mut Option<price::DryRunAdmissions>),
}

async fn ingest_file(
    db: &mut tokio_postgres::Client,
    feed: Feed,
    ingestion: FileIngestion<'_>,
    file: DownloadedFile,
    tracked: &std::collections::HashSet<String>,
    eligible: &std::collections::BTreeSet<(UserId, String)>,
    scope_hash: &str,
) -> anyhow::Result<usize> {
    const BATCH_SIZE: usize = 4096;
    let DownloadedFile { file, bytes, etag, content_hash, permit } = file;
    let file_key = file.key.clone();
    enum Destination<'a> {
        Persist(price::FileWriter<'a>),
        DryRun {
            client: &'a tokio_postgres::Client,
            state: price::DryRunAdmissions,
            resume: &'a mut Option<price::DryRunAdmissions>,
            admitted: usize,
        },
    }
    let mut destination = match ingestion {
        FileIngestion::Persist(job) => Destination::Persist(price::begin_file(db, job, &file_key, tracked, eligible, scope_hash).await?),
        FileIngestion::DryRun(resume) => Destination::DryRun {
            client: db,
            state: resume.take().context("Dry-run refresh already aborted")?,
            resume,
            admitted: 0,
        },
    };
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    let (recycle, mut available) = tokio::sync::mpsc::channel(2);
    // Three reusable batches bound all-market parsing independently of file size.
    for _ in 0..2 {
        recycle.try_send(Vec::<types::PriceReport>::with_capacity(BATCH_SIZE))?;
    }
    let parse = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let parsed = {
            let mut emit = |report| -> anyhow::Result<()> {
                batch.push(report);
                if batch.len() == BATCH_SIZE {
                    sender.blocking_send(std::mem::take(&mut batch))
                        .map_err(|_| anyhow::anyhow!("Price ingestion cancelled"))?;
                    batch = available.blocking_recv()
                        .context("Price ingestion cancelled")?;
                }
                Ok(())
            };
            adapter::parse(feed, &file, &bytes, Utc::now(), &mut emit)
                .with_context(|| format!("Parsing {}", file.key))?
        };
        if !batch.is_empty() {
            sender.blocking_send(batch)
                .map_err(|_| anyhow::anyhow!("Price ingestion cancelled"))?;
        }
        Ok::<_, anyhow::Error>(parsed)
    });
    while let Some(mut batch) = receiver.recv().await {
        match &mut destination {
            Destination::Persist(writer) => writer.push(&batch).await?,
            Destination::DryRun { client, state, admitted, .. } => {
                *admitted += state.push(client, &batch, tracked).await?;
            }
        }
        batch.clear();
        // Once parsing finishes, unused recycled storage can be dropped.
        let _ = recycle.try_send(batch);
    }
    parse.await.context("Price parser task stopped")??;
    match destination {
        Destination::Persist(writer) => writer.finish(&content_hash, etag.as_deref()).await,
        Destination::DryRun { state, resume, admitted, .. } => {
            *resume = Some(state);
            Ok(admitted)
        },
    }
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

pub async fn status(user_id: &UserId) -> anyhow::Result<PricingStatus> {
    let db = db_client().await?;
    let config = price_scheduler::read(&db).await?;
    let isins: Vec<_> = get_positions(user_id, None, None, None)
        .await?
        .into_iter()
        .map(|p| p.isin)
        .collect();
    Ok(PricingStatus {
        enabled: config.enabled,
        interval_minutes: config.interval_minutes,
        worker_concurrency: config.workers as u32,
        feeds: price::feed_status(&db).await?,
        quotes: price::quotes(user_id, &db, &isins).await?,
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
