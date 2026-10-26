use anyhow::{ensure, Context};
use tokio_postgres::Client;

use crate::services::market_data::prices::config::{Feed, PriceConfig, MAX_WORKERS};

use super::background_job;

fn validate(
    enabled: bool,
    interval_minutes: u32,
    workers: usize,
    feeds: &[Feed],
) -> anyhow::Result<()> {
    ensure!(
        (1..=1440).contains(&interval_minutes),
        "Price interval must be 1..1440 minutes"
    );
    ensure!(
        (1..=MAX_WORKERS).contains(&workers),
        "Price worker concurrency must be 1..4"
    );
    ensure!(
        !enabled || !feeds.is_empty(),
        "Select at least one price feed before enabling automatic refresh"
    );
    Ok(())
}

/// Seed built-in defaults once; subsequent starts only ensure known schedules exist.
pub async fn initialize(client: &mut Client) -> anyhow::Result<()> {
    let config = PriceConfig::default();
    let interval_minutes = i32::try_from(config.interval_minutes)?;
    let tx = client.transaction().await?;
    // The conflicting insert also serializes concurrent first starts before any feed locks.
    let inserted = tx
        .execute(
            "INSERT INTO price_scheduler_settings(singleton, enabled, interval_minutes)
             VALUES(TRUE, $1, $2) ON CONFLICT(singleton) DO NOTHING",
            &[&config.enabled, &interval_minutes],
        )
        .await?;
    tx.query_one(
        "SELECT singleton FROM price_scheduler_settings WHERE singleton FOR UPDATE",
        &[],
    )
    .await?;
    let known = Feed::ALL.map(Feed::key);
    tx.execute(
        "INSERT INTO price_feed_schedule(feed)
         SELECT feed FROM unnest($1::text[]) AS feed ORDER BY feed ON CONFLICT DO NOTHING",
        &[&known.as_slice()],
    )
    .await?;
    if inserted != 0 {
        tx.query(
            "SELECT feed FROM price_feed_schedule ORDER BY feed FOR UPDATE",
            &[],
        )
        .await?;
        let selected: Vec<_> = config.feeds.iter().map(|feed| feed.key()).collect();
        tx.execute(
            "UPDATE price_feed_schedule SET enabled = feed = ANY($1),
                next_refresh_at = CASE WHEN feed = ANY($1) AND NOT enabled
                    THEN clock_timestamp() ELSE next_refresh_at END",
            &[&selected],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Read the global switch, interval and feed selection from one database snapshot.
pub async fn read(client: &Client) -> anyhow::Result<PriceConfig> {
    let known = Feed::ALL.map(Feed::key);
    let rows = client
        .query(
            "SELECT settings.enabled, settings.interval_minutes, schedule.feed, settings.worker_concurrency
             FROM price_scheduler_settings settings
             LEFT JOIN price_feed_schedule schedule
                ON schedule.enabled AND schedule.feed = ANY($1::text[])
             WHERE settings.singleton
             ORDER BY array_position($1::text[], schedule.feed)",
            &[&known.as_slice()],
        )
        .await?;
    let settings = rows
        .first()
        .context("Price scheduler has not been initialized")?;
    let mut config = PriceConfig {
        enabled: settings.get(0),
        interval_minutes: u32::try_from(settings.get::<_, i32>(1))?,
        workers: usize::try_from(settings.get::<_, i32>(3))?,
        feeds: Vec::with_capacity(rows.len()),
    };
    for row in rows {
        if let Some(feed) = row.get::<_, Option<&str>>(2) {
            config.feeds.push(Feed::from_key(feed)?);
        }
    }
    Ok(config)
}

pub async fn controls(client: &Client) -> anyhow::Result<(usize, String)> {
    let row = client
        .query_one(
            "SELECT s.worker_concurrency, a.log_level FROM price_scheduler_settings s
         CROSS JOIN application_settings a WHERE s.singleton AND a.singleton",
            &[],
        )
        .await?;
    Ok((usize::try_from(row.get::<_, i32>(0))?, row.get(1)))
}

pub async fn update(
    client: &mut Client,
    enabled: bool,
    interval_minutes: u32,
    workers: usize,
    feeds: &[Feed],
) -> anyhow::Result<()> {
    validate(enabled, interval_minutes, workers, feeds)?;
    let interval_minutes = i32::try_from(interval_minutes)?;
    let workers = i32::try_from(workers)?;
    let tx = client.transaction().await?;
    let previous = tx
        .query_one(
            "SELECT enabled, interval_minutes FROM price_scheduler_settings
             WHERE singleton FOR UPDATE",
            &[],
        )
        .await?;
    let resumed = enabled && !previous.get::<_, bool>(0);
    let interval_changed = interval_minutes != previous.get::<_, i32>(1);
    let mut selected: Vec<_> = feeds.iter().map(|feed| feed.key()).collect();
    selected.sort_unstable();
    selected.dedup();
    // The singleton serializes selection writers. Read after acquiring it so
    // a save that waited for another save sees that transaction's final selection.
    let previous_feeds = tx
        .query(
            "SELECT feed FROM price_feed_schedule WHERE enabled ORDER BY feed",
            &[],
        )
        .await?;
    let selection_changed = previous_feeds
        .iter()
        .map(|row| row.get::<_, &str>(0))
        .ne(selected.iter().copied());
    if enabled != previous.get::<_, bool>(0) || interval_changed || selection_changed {
        // Only schedule changes need feed locks. Concurrency-only saves must not
        // wait behind a finishing import's feed lock.
        tx.query(
            "SELECT feed FROM price_feed_schedule ORDER BY feed FOR UPDATE",
            &[],
        )
        .await?;
        tx.execute(
            "UPDATE price_feed_schedule SET enabled = feed = ANY($1),
                next_refresh_at = CASE
                    WHEN feed = ANY($1) AND ($2 OR NOT enabled) THEN clock_timestamp()
                    WHEN feed = ANY($1) AND enabled AND $3
                        THEN clock_timestamp() + $4::integer * interval '1 minute'
                    ELSE next_refresh_at END",
            &[&selected, &resumed, &interval_changed, &interval_minutes],
        )
        .await?;
    }
    tx.execute(
        "UPDATE price_scheduler_settings SET enabled = $1, interval_minutes = $2, worker_concurrency = $3 WHERE singleton",
        &[&enabled, &interval_minutes, &workers],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn schedule_due(client: &mut Client) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    // A pause save waits for this lock, so it cannot return before a scheduling tick ends.
    let settings = tx
        .query_one(
            "SELECT enabled, interval_minutes FROM price_scheduler_settings
             WHERE singleton FOR SHARE",
            &[],
        )
        .await?;
    if settings.get::<_, bool>(0) {
        let interval_minutes: i32 = settings.get(1);
        let known = Feed::ALL.map(Feed::key);
        let rows = tx
            .query(
                "SELECT feed FROM price_feed_schedule
                 WHERE enabled AND feed = ANY($1::text[]) AND next_refresh_at <= clock_timestamp()
                 ORDER BY feed FOR UPDATE SKIP LOCKED",
                &[&known.as_slice()],
            )
            .await?;
        for row in rows {
            let feed: &str = row.get(0);
            background_job::enqueue_in(&tx, feed, false).await?;
            // Coalesce downtime into one catch-up job rather than replaying missed slots.
            tx.execute(
                "UPDATE price_feed_schedule
                 SET next_refresh_at = clock_timestamp() + $2::integer * interval '1 minute'
                 WHERE feed = $1",
                &[&feed, &interval_minutes],
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Replace only this feed's active job; imported data and terminal job history remain intact.
pub async fn reset(client: &mut Client, feed: Feed) -> anyhow::Result<i64> {
    let tx = client.transaction().await?;
    tx.execute(
        "INSERT INTO price_feed_schedule(feed) VALUES($1) ON CONFLICT DO NOTHING",
        &[&feed.key()],
    )
    .await?;
    // Match enqueue/finish/fail: feed schedule first, then its active job.
    tx.query_one(
        "SELECT feed FROM price_feed_schedule WHERE feed = $1 FOR UPDATE",
        &[&feed.key()],
    )
    .await?;
    // Status and lease predicates fence renew/commit/finish/fail/release after this commits.
    tx.execute(
        "UPDATE background_job SET status = 'failed', finished_at = clock_timestamp(),
            lease_until = NULL, rerun_requested = FALSE,
            last_error = 'Reset manually; replaced by a fresh import'
         WHERE feed = $1 AND status IN ('queued', 'running')",
        &[&feed.key()],
    )
    .await?;
    let id = background_job::enqueue_in(&tx, feed.key(), false).await?;
    tx.execute(
        "UPDATE price_feed_schedule SET last_error = NULL WHERE feed = $1",
        &[&feed.key()],
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}
