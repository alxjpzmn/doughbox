use anyhow::{ensure, Context};
use tokio_postgres::{Client, Transaction};

use crate::services::market_data::prices::{config::Feed, types::PriceJob};

pub const LEASE_SECONDS: i32 = 300;
const MAX_ATTEMPTS: i32 = 4;
const RETENTION_BATCH_SIZE: i64 = 5_000;

const EXPIRE_EXHAUSTED: &str = "WITH exhausted AS (
    SELECT id FROM background_job WHERE status = 'running' AND lease_until <= clock_timestamp()
        AND attempts >= $1 AND feed = ANY($2) ORDER BY feed FOR UPDATE SKIP LOCKED)
    UPDATE background_job j SET status = 'failed', finished_at = clock_timestamp(), lease_until = NULL,
        last_error = 'Worker lease expired after the final attempt'
    FROM exhausted e WHERE j.id = e.id";

pub async fn ensure_feeds(client: &Client, feeds: &[Feed]) -> anyhow::Result<()> {
    for feed in feeds {
        client
            .execute(
                "INSERT INTO price_feed_schedule(feed) VALUES($1) ON CONFLICT DO NOTHING",
                &[&feed.key()],
            )
            .await?;
    }
    Ok(())
}

pub(super) async fn enqueue_in(
    tx: &Transaction<'_>,
    feed: &str,
    manual: bool,
) -> anyhow::Result<i64> {
    let row = tx.query_one(
        "INSERT INTO background_job(feed) VALUES($1)
         ON CONFLICT(feed) WHERE status IN ('queued', 'running') DO UPDATE SET
            rerun_requested = background_job.rerun_requested OR ($2 AND background_job.status = 'running')
         RETURNING id", &[&feed, &manual]).await?;
    Ok(row.get(0))
}

pub async fn enqueue(client: &mut Client, feeds: &[Feed]) -> anyhow::Result<Vec<i64>> {
    ensure_feeds(client, feeds).await?;
    let tx = client.transaction().await?;
    let mut ids = Vec::new();
    // Match the scheduler's key order even if CLI/configuration order differs.
    // Otherwise concurrent multi-feed enqueues can lock active jobs in reverse order.
    let mut feeds = feeds.to_vec();
    feeds.sort_by_key(|feed| feed.key());
    feeds.dedup();
    for feed in feeds {
        // Match finish/scheduler lock order before retiring a job and inserting its replacement.
        tx.query_one(
            "SELECT feed FROM price_feed_schedule WHERE feed = $1 FOR UPDATE",
            &[&feed.key()],
        )
        .await?;
        let keys = [feed.key()];
        tx.execute(EXPIRE_EXHAUSTED, &[&MAX_ATTEMPTS, &keys.as_slice()])
            .await?;
        ids.push(enqueue_in(&tx, feed.key(), true).await?);
    }
    tx.commit().await?;
    Ok(ids)
}

pub async fn claim(client: &Client, feeds: &[Feed]) -> anyhow::Result<Option<PriceJob>> {
    let feeds: Vec<_> = feeds.iter().map(|feed| feed.key()).collect();
    // Exhausted leases must not leave a feed blocked forever after repeated crashes.
    client
        .execute(EXPIRE_EXHAUSTED, &[&MAX_ATTEMPTS, &feeds])
        .await?;
    let row = client.query_opt("WITH candidate AS (
        SELECT id FROM background_job
        WHERE feed = ANY($1) AND attempts < $2 AND
            ((status = 'queued' AND available_at <= now()) OR (status = 'running' AND lease_until <= now()))
        ORDER BY available_at, id FOR UPDATE SKIP LOCKED LIMIT 1)
        UPDATE background_job j SET status = 'running', attempts = attempts + 1,
            lease_token = lease_token + 1, lease_until = now() + $3::integer * interval '1 second'
        FROM candidate c WHERE j.id = c.id
        RETURNING j.id, j.feed, j.attempts, j.lease_token", &[&feeds, &MAX_ATTEMPTS, &LEASE_SECONDS]).await?;
    Ok(row.map(|row| PriceJob {
        id: row.get(0),
        feed: row.get(1),
        attempts: row.get(2),
        lease_token: row.get(3),
    }))
}

pub async fn renew(client: &Client, job: &PriceJob) -> anyhow::Result<()> {
    let owned: bool = client.query_one(
        "WITH unlocked AS MATERIALIZED (
            SELECT id FROM background_job WHERE id = $1 FOR UPDATE SKIP LOCKED),
         renewed AS (
            UPDATE background_job j SET lease_until = clock_timestamp() + $3::integer * interval '1 second'
            FROM unlocked WHERE j.id = unlocked.id AND j.lease_token = $2
                AND j.status = 'running' AND j.lease_until > clock_timestamp()
            RETURNING j.id)
         SELECT EXISTS(SELECT 1 FROM renewed) OR
            (NOT EXISTS(SELECT 1 FROM unlocked) AND EXISTS(
                SELECT 1 FROM background_job WHERE id = $1 AND lease_token = $2 AND status = 'running'))",
        &[&job.id, &job.lease_token, &LEASE_SECONDS],
    ).await?.get(0);
    // A file writer renews inside its transaction while holding this row lock.
    // Its newer deadline is invisible here until commit, so only the unlocked
    // path checks expiry; both paths still fence status and ownership.
    ensure!(owned, "Price job {} lost its lease", job.id);
    Ok(())
}

/// Return an intentionally interrupted job to the queue without consuming a retry.
pub async fn release(client: &Client, job: &PriceJob) -> anyhow::Result<()> {
    let changed = client
        .execute(
            "UPDATE background_job SET status = 'queued', attempts = attempts - 1,
                lease_until = NULL, available_at = clock_timestamp()
             WHERE id = $1 AND lease_token = $2 AND status = 'running'
                AND lease_until > clock_timestamp()",
            &[&job.id, &job.lease_token],
        )
        .await?;
    ensure!(changed == 1, "Price job {} lost its lease", job.id);
    Ok(())
}

pub async fn lock_lease(tx: &Transaction<'_>, job: &PriceJob) -> anyhow::Result<()> {
    let row = tx
        .query_opt(
            "SELECT id FROM background_job WHERE id = $1 AND lease_token = $2
        AND status = 'running' AND lease_until > clock_timestamp() FOR UPDATE",
            &[&job.id, &job.lease_token],
        )
        .await?;
    ensure!(row.is_some(), "Price job {} lost its lease", job.id);
    Ok(())
}

pub async fn finish(client: &mut Client, job: &PriceJob) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    // Match the scheduler's lock order (schedule, then job) to avoid a cycle.
    tx.query_one(
        "SELECT feed FROM price_feed_schedule WHERE feed = $1 FOR UPDATE",
        &[&job.feed],
    )
    .await?;
    lock_lease(&tx, job).await?;
    let rerun: bool = tx
        .query_one(
            "UPDATE background_job SET status = 'succeeded', finished_at = now(),
        lease_until = NULL, last_error = NULL WHERE id = $1 RETURNING rerun_requested",
            &[&job.id],
        )
        .await?
        .get(0);
    tx.execute(
        "UPDATE price_feed_schedule SET last_success_at = now(), last_error = NULL WHERE feed = $1",
        &[&job.feed],
    )
    .await?;
    if rerun {
        enqueue_in(&tx, &job.feed, false).await?;
    }
    prune_in(&tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Prune one bounded batch without requiring a successful feed job.
/// Returns whether another batch may be needed.
pub async fn prune(client: &mut Client) -> anyhow::Result<bool> {
    let tx = client.transaction().await?;
    let more = prune_in(&tx).await?;
    tx.commit().await?;
    Ok(more)
}

pub(crate) async fn prune_in(tx: &Transaction<'_>) -> anyhow::Result<bool> {
    tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&super::price::PROJECTION_LOCK]).await?;
    let quotes = tx.execute(
        "WITH expired AS MATERIALIZED (
            SELECT isin, currency, feed FROM mifid_latest_price
            WHERE as_of < now() - interval '168 hours'
                AND executed_ns < (extract(epoch FROM now() - interval '168 hours') * 1000000000)::bigint
            ORDER BY as_of, isin, currency, feed LIMIT $1 FOR UPDATE SKIP LOCKED)
         DELETE FROM mifid_latest_price q USING expired e
         WHERE (q.isin, q.currency, q.feed) = (e.isin, e.currency, e.feed)",
        &[&RETENTION_BATCH_SIZE],
    ).await?;
    let floors = tx.execute(
        "WITH expired AS MATERIALIZED (
            SELECT isin, currency, feed FROM mifid_price_floor
            WHERE executed_ns < (extract(epoch FROM now() - interval '168 hours') * 1000000000)::bigint
            ORDER BY executed_ns, isin, currency, feed LIMIT $1 FOR UPDATE SKIP LOCKED)
         DELETE FROM mifid_price_floor f USING expired e
         WHERE (f.isin, f.currency, f.feed) = (e.isin, e.currency, e.feed)",
        &[&RETENTION_BATCH_SIZE],
    ).await?;
    let revisions = tx.execute(
        "WITH expired AS MATERIALIZED (
            SELECT id FROM mifid_price_revision
            WHERE published_at < now() - interval '7 days'
                AND published_ns < (extract(epoch FROM now() - interval '7 days') * 1000000000)::bigint
            ORDER BY published_at, id LIMIT $1 FOR UPDATE SKIP LOCKED)
         DELETE FROM mifid_price_revision r USING expired e WHERE r.id = e.id",
        &[&RETENTION_BATCH_SIZE],
    ).await?;
    // Importers lock source rows before taking the projection lock. Skip those
    // rows rather than deadlocking; portfolio references remain protected.
    let reports = tx.execute(
        "WITH expired AS MATERIALIZED (
            SELECT r.id FROM market_price_report r
            WHERE r.received_at < now() - interval '7 days'
                AND NOT EXISTS (SELECT 1 FROM instrument i WHERE i.price_report_id = r.id)
            ORDER BY r.received_at, r.id LIMIT $1 FOR UPDATE OF r SKIP LOCKED)
         DELETE FROM market_price_report r USING expired WHERE r.id = expired.id",
        &[&RETENTION_BATCH_SIZE],
    ).await?;
    let files = tx.execute(
        "WITH expired AS MATERIALIZED (
            SELECT feed, file_key, parser_version, scope_hash FROM market_data_file
            WHERE processed_at < now() - interval '7 days'
            ORDER BY processed_at LIMIT $1 FOR UPDATE SKIP LOCKED)
         DELETE FROM market_data_file f USING expired e
         WHERE f.feed = e.feed AND f.file_key = e.file_key
            AND f.parser_version = e.parser_version AND f.scope_hash = e.scope_hash",
        &[&RETENTION_BATCH_SIZE],
    ).await?;
    let jobs = tx.execute(
        "WITH expired AS MATERIALIZED (
            SELECT id FROM background_job
            WHERE status IN ('succeeded', 'failed') AND finished_at < now() - interval '14 days'
            ORDER BY finished_at LIMIT $1 FOR UPDATE SKIP LOCKED)
         DELETE FROM background_job j USING expired e WHERE j.id = e.id",
        &[&RETENTION_BATCH_SIZE],
    ).await?;
    Ok([quotes, floors, revisions, reports, files, jobs].into_iter()
        .any(|count| count == RETENTION_BATCH_SIZE as u64))
}

pub async fn fail(
    client: &mut Client,
    job: &PriceJob,
    error: &str,
    retry_seconds: i32,
) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    tx.query_one(
        "SELECT feed FROM price_feed_schedule WHERE feed = $1 FOR UPDATE",
        &[&job.feed],
    )
    .await?;
    lock_lease(&tx, job).await?;
    // Bound upstream error messages retained in the database.
    let error: String = error.chars().take(4000).collect();
    tx.execute("UPDATE background_job SET status = CASE WHEN attempts >= $3 THEN 'failed' ELSE 'queued' END,
        finished_at = CASE WHEN attempts >= $3 THEN now() ELSE NULL END, lease_until = NULL,
        available_at = now() + $4::integer * interval '1 second', last_error = $5 WHERE id = $1 AND lease_token = $2",
        &[&job.id, &job.lease_token, &MAX_ATTEMPTS, &retry_seconds, &error]).await?;
    tx.execute(
        "UPDATE price_feed_schedule SET last_error = $2 WHERE feed = $1",
        &[&job.feed, &error],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn job_states(
    client: &Client,
    ids: &[i64],
) -> anyhow::Result<Vec<(String, Option<String>)>> {
    let rows = client
        .query(
            "SELECT status, last_error FROM background_job WHERE id = ANY($1) ORDER BY id",
            &[&ids],
        )
        .await?;
    ensure!(
        rows.len() == ids.len(),
        "Price jobs disappeared before completion"
    );
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get(0).context("Missing job status")?,
                row.try_get(1)?,
            ))
        })
        .collect()
}
