use chrono::{DateTime, NaiveDate, Utc};
use tokio_postgres::{Client, GenericClient, Row};

use crate::services::calculations::CalculationStatus;

// Distinct from import and price-worker locks. Each worker retains its own
// PostgreSQL session, so separate API processes coalesce on the same source.
const LOCK_NAMESPACE: i32 = 0x44424341;
const INTERRUPTED_ERROR: &str =
    "Calculation interrupted before completion. Run the calculation again to retry.";

fn source_key(source: &str) -> anyhow::Result<i32> {
    match source {
        "performance" => Ok(1),
        "taxation" => Ok(2),
        _ => anyhow::bail!("Unsupported calculation source"),
    }
}

fn status_from_row(row: Row) -> CalculationStatus {
    CalculationStatus {
        source: row.get("source"),
        status: row.get("status"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        last_success_at: row.get("last_success_at"),
        last_error: row.get("last_error"),
        from_date: row.get("from_date"),
        until_date: row.get("until_date"),
        schedule_enabled: row.get("schedule_enabled"),
        schedule_frequency: row.get("schedule_frequency"),
        next_refresh_at: row.get("next_refresh_at"),
    }
}

async fn try_lock(db: &(impl GenericClient + Sync), source: &str) -> anyhow::Result<bool> {
    let key = source_key(source)?;
    Ok(db
        .query_one(
            "SELECT pg_try_advisory_lock($1, $2)",
            &[&LOCK_NAMESPACE, &key],
        )
        .await?
        .get(0))
}

pub async fn unlock(db: &Client, source: &str) -> anyhow::Result<()> {
    let key = source_key(source)?;
    db.query_one(
        "SELECT pg_advisory_unlock($1, $2)",
        &[&LOCK_NAMESPACE, &key],
    )
    .await?;
    Ok(())
}

/// Saves serialize on the same row as claims. Identical saves retain their
/// deadline; manual claims and outcomes never update schedule configuration.
pub async fn update_schedule(
    db: &Client,
    source: &str,
    enabled: bool,
    frequency: &str,
) -> anyhow::Result<CalculationStatus> {
    source_key(source)?;
    anyhow::ensure!(
        matches!(frequency, "daily" | "weekly" | "monthly"),
        "Unsupported calculation frequency"
    );
    let row = db
        .query_one(
            "UPDATE calculation_status SET schedule_enabled = $2, schedule_frequency = $3,
             next_refresh_at = CASE
                 WHEN NOT $2 THEN NULL
                 WHEN NOT schedule_enabled OR schedule_frequency <> $3 THEN clock_timestamp()
                 ELSE next_refresh_at END
             WHERE source = $1 RETURNING *",
            &[&source, &enabled, &frequency],
        )
        .await?;
    Ok(status_from_row(row))
}

/// A due slot is consumed only after taking the manual worker's source lock.
/// The row lock serializes configuration changes and replica claims, and the
/// recheck keeps stale ticks from undoing a pause or using an old cadence.
pub async fn try_claim_scheduled(
    db: &mut Client,
    source: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    source_key(source)?;
    let tx = db.transaction().await?;
    let current = tx
        .query_one(
            "SELECT * FROM calculation_status WHERE source = $1 FOR UPDATE",
            &[&source],
        )
        .await?;
    let enabled: bool = current.get("schedule_enabled");
    let next: Option<DateTime<Utc>> = current.get("next_refresh_at");
    if !enabled || next.is_none_or(|next| next > now) || !try_lock(&tx, source).await? {
        tx.commit().await?;
        return Ok(false);
    }
    tx.execute(
        "UPDATE calculation_status SET status = 'running', started_at = $2,
             finished_at = NULL, last_error = NULL, from_date = NULL, until_date = NULL,
             next_refresh_at = (
                 ($2::timestamptz AT TIME ZONE 'UTC') +
                 CASE schedule_frequency
                     WHEN 'daily' THEN interval '1 day'
                     WHEN 'weekly' THEN interval '1 week'
                     WHEN 'monthly' THEN interval '1 month'
                 END
             ) AT TIME ZONE 'UTC'
         WHERE source = $1",
        &[&source, &now],
    )
    .await?;
    // Coalesce missed slots into one run, advancing from the claim time, not
    // the old deadline. PostgreSQL clamps UTC calendar months at month-end.
    tx.commit().await?;
    Ok(true)
}

/// Claim before spawning. A successful caller owns this dedicated session lock
/// until finish; a coalesced request retains the active calculation's range.
/// The row lock also makes coalescing wait for an in-progress claim to publish
/// 'running', instead of returning an obsolete 'never_run' or terminal state.
pub async fn try_claim(
    db: &mut Client,
    source: &str,
    from_date: Option<NaiveDate>,
    until_date: Option<NaiveDate>,
) -> anyhow::Result<(bool, CalculationStatus)> {
    source_key(source)?;
    let tx = db.transaction().await?;
    let current = tx
        .query_one(
            "SELECT * FROM calculation_status WHERE source = $1 FOR UPDATE",
            &[&source],
        )
        .await?;
    if !try_lock(&tx, source).await? {
        tx.commit().await?;
        return Ok((false, status_from_row(current)));
    }
    let row = tx
        .query_one(
            "UPDATE calculation_status SET status = 'running', started_at = clock_timestamp(),
             finished_at = NULL, last_error = NULL, from_date = $2, until_date = $3
             WHERE source = $1 RETURNING *",
            &[&source, &from_date, &until_date],
        )
        .await?;
    tx.commit().await?;
    Ok((true, status_from_row(row)))
}

/// Recovery only changes an abandoned running record. It cannot reclaim a live
/// worker in this or another API process, and it never runs financial work.
pub async fn recover_interrupted(
    db: &mut Client,
    source: &str,
) -> anyhow::Result<CalculationStatus> {
    source_key(source)?;
    let tx = db.transaction().await?;
    let current = tx
        .query_one(
            "SELECT * FROM calculation_status WHERE source = $1 FOR UPDATE",
            &[&source],
        )
        .await?;
    if !try_lock(&tx, source).await? {
        tx.commit().await?;
        return Ok(status_from_row(current));
    }
    let recovered = tx
        .query_opt(
            "UPDATE calculation_status SET status = 'failed', finished_at = clock_timestamp(),
             last_error = $2 WHERE source = $1 AND status = 'running' RETURNING *",
            &[&source, &INTERRUPTED_ERROR],
        )
        .await?;
    tx.commit().await?;
    unlock(db, source).await?;
    Ok(status_from_row(recovered.unwrap_or(current)))
}

/// Only the worker retaining this source's advisory-lock session may finish it.
/// A failed refresh preserves the timestamp of the last successful calculation.
pub async fn finish(db: &Client, source: &str, error: Option<&str>) -> anyhow::Result<()> {
    source_key(source)?;
    db.execute(
        "UPDATE calculation_status
         SET status = CASE WHEN $2::text IS NULL THEN 'succeeded' ELSE 'failed' END,
             finished_at = clock_timestamp(), last_error = $2,
             last_success_at = CASE WHEN $2::text IS NULL THEN clock_timestamp() ELSE last_success_at END
         WHERE source = $1 AND status = 'running'",
        &[&source, &error],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use tokio_postgres::NoTls;

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn claims_coalesce_preserve_live_runs_and_recover_lost_sessions() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("Set an isolated TEST_POSTGRES_URL");
        let (mut first, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        let first_connection = tokio::spawn(connection);
        let schema = format!(
            "calculations_test_{}_{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        );
        first
            .batch_execute(&format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            ))
            .await
            .unwrap();
        refinery::embed_migrations!("migrations");
        migrations::runner().run_async(&mut first).await.unwrap();
        let (mut second, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        let second_connection = tokio::spawn(connection);
        second
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .unwrap();

        let initial = recover_interrupted(&mut second, "performance").await.unwrap();
        assert_eq!(initial.status, "never_run");
        assert!(initial.started_at.is_none());
        assert!(initial.last_success_at.is_none());
        assert!(!initial.schedule_enabled);
        assert_eq!(initial.schedule_frequency, "monthly");
        assert!(initial.next_refresh_at.is_none());

        // Both requests race before either spawns financial work. Exactly one
        // owns the lock; both responses describe that same committed run.
        let (a, b) = tokio::join!(
            try_claim(&mut first, "performance", None, None),
            try_claim(&mut second, "performance", None, None)
        );
        let (a_claimed, a) = a.unwrap();
        let (b_claimed, b) = b.unwrap();
        assert_ne!(a_claimed, b_claimed);
        assert_eq!(a.status, "running");
        assert_eq!(b.status, "running");
        assert_eq!(a.started_at, b.started_at);
        let owner = if a_claimed { &first } else { &second };
        finish(owner, "performance", None).await.unwrap();
        unlock(owner, "performance").await.unwrap();
        let succeeded = recover_interrupted(&mut second, "performance").await.unwrap();
        assert_eq!(succeeded.status, "succeeded");
        let last_success = succeeded.last_success_at.unwrap();

        let (claimed, running) = try_claim(&mut first, "performance", None, None).await.unwrap();
        assert!(claimed);
        assert_eq!(running.last_success_at, Some(last_success));
        assert!(running.finished_at.is_none());
        assert!(running.last_error.is_none());
        let live = recover_interrupted(&mut second, "performance").await.unwrap();
        assert_eq!(live.status, "running");
        assert_eq!(live.started_at, running.started_at);
        assert!(live.finished_at.is_none());

        // Tax work is independent of a live performance calculation, and
        // duplicate requests cannot relabel the running tax report's range.
        let from = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        let (claimed, tax) = try_claim(&mut second, "taxation", Some(from), Some(until)).await.unwrap();
        assert!(claimed);
        assert_eq!(tax.from_date, Some(from));
        assert_eq!(tax.until_date, Some(until));
        let (claimed, coalesced) = try_claim(&mut first, "taxation", None, None).await.unwrap();
        assert!(!claimed);
        assert_eq!(coalesced.started_at, tax.started_at);
        assert_eq!(coalesced.from_date, Some(from));
        assert_eq!(coalesced.until_date, Some(until));
        finish(&second, "taxation", None).await.unwrap();
        unlock(&second, "taxation").await.unwrap();
        let tax_succeeded = recover_interrupted(&mut second, "taxation").await.unwrap();
        assert_eq!(tax_succeeded.status, "succeeded");
        assert_eq!(tax_succeeded.from_date, Some(from));
        assert_eq!(tax_succeeded.until_date, Some(until));

        // Losing the actual owning connection simulates API process death.
        drop(first);
        first_connection.await.unwrap().unwrap();
        let interrupted = recover_interrupted(&mut second, "performance").await.unwrap();
        assert_eq!(interrupted.status, "failed");
        assert!(interrupted.finished_at.is_some());
        assert_eq!(interrupted.last_error.as_deref(), Some(INTERRUPTED_ERROR));
        assert_eq!(interrupted.last_success_at, Some(last_success));
        assert!(interrupted.from_date.is_none());
        assert!(interrupted.until_date.is_none());

        let (claimed, retry) = try_claim(&mut second, "performance", None, None).await.unwrap();
        assert!(claimed);
        assert!(retry.last_error.is_none());
        assert!(retry.finished_at.is_none());
        finish(&second, "performance", Some("Performance refresh failed.")).await.unwrap();
        unlock(&second, "performance").await.unwrap();
        let failed = recover_interrupted(&mut second, "performance").await.unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.last_success_at, Some(last_success));

        let (claimed, all_history) = try_claim(&mut second, "taxation", None, None).await.unwrap();
        assert!(claimed);
        assert!(all_history.from_date.is_none());
        assert!(all_history.until_date.is_none());
        assert_eq!(all_history.last_success_at, tax_succeeded.last_success_at);
        finish(&second, "taxation", Some("Taxation recalculation failed.")).await.unwrap();
        unlock(&second, "taxation").await.unwrap();
        let failed_tax = recover_interrupted(&mut second, "taxation").await.unwrap();
        assert_eq!(failed_tax.status, "failed");
        assert_eq!(failed_tax.last_success_at, tax_succeeded.last_success_at);
        let performance = recover_interrupted(&mut second, "performance").await.unwrap();
        assert_eq!(performance.last_success_at, Some(last_success));
        assert_eq!(performance.status, "failed");

        // Keep every advisory-lock scenario in one test; the namespace is
        // database-wide even though each test schema is isolated.
        exercise_schedule_claims(&mut second, &url, &schema).await;
        second
            .batch_execute(&format!(
                "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
            ))
            .await
            .unwrap();
        drop(second);
        second_connection.await.unwrap().unwrap();
    }

    async fn stored_status(db: &Client, source: &str) -> CalculationStatus {
        status_from_row(
            db.query_one("SELECT * FROM calculation_status WHERE source = $1", &[&source])
                .await
                .unwrap(),
        )
    }

    async fn make_due(db: &Client, source: &str, deadline: DateTime<Utc>) {
        db.execute(
            "UPDATE calculation_status SET next_refresh_at = $2 WHERE source = $1",
            &[&source, &deadline],
        )
        .await
        .unwrap();
    }

    async fn exercise_schedule_claims(owner: &mut Client, url: &str, schema: &str) {
        let (mut replica, connection) = tokio_postgres::connect(url, NoTls).await.unwrap();
        let replica_connection = tokio::spawn(connection);
        replica
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .unwrap();
        let now: DateTime<Utc> = "2100-01-31T23:30:00Z".parse().unwrap();
        for source in ["performance", "taxation"] {
            let status = stored_status(&replica, source).await;
            assert!(!status.schedule_enabled);
            assert_eq!(status.schedule_frequency, "monthly");
            assert!(status.next_refresh_at.is_none());
            assert!(!try_claim_scheduled(owner, source, now).await.unwrap());
        }

        // Disabled manual work remains possible, without enabling a schedule.
        let from = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        let (claimed, manual) = try_claim(owner, "taxation", Some(from), Some(until)).await.unwrap();
        assert!(claimed);
        assert!(!manual.schedule_enabled);
        assert!(manual.next_refresh_at.is_none());
        let enabled = update_schedule(&replica, "taxation", true, "monthly").await.unwrap();
        assert_eq!(enabled.status, "running");
        assert_eq!(enabled.from_date, Some(from));
        assert_eq!(enabled.until_date, Some(until));
        assert!(enabled.schedule_enabled);
        assert!(enabled.next_refresh_at.unwrap() <= Utc::now());
        let unchanged = update_schedule(owner, "taxation", true, "monthly").await.unwrap();
        assert_eq!(unchanged.next_refresh_at, enabled.next_refresh_at);

        let future = now + TimeDelta::days(400);
        make_due(owner, "taxation", future).await;
        let unchanged = update_schedule(&replica, "taxation", true, "monthly").await.unwrap();
        assert_eq!(unchanged.next_refresh_at, Some(future));
        let changed = update_schedule(&replica, "taxation", true, "weekly").await.unwrap();
        assert_eq!(changed.schedule_frequency, "weekly");
        assert!(changed.next_refresh_at.unwrap() <= Utc::now());
        assert!(update_schedule(owner, "taxation", true, "hourly").await.is_err());
        assert!(update_schedule(owner, "unknown", true, "daily").await.is_err());

        // A busy scheduled slot cannot advance or overwrite a manual range.
        make_due(owner, "taxation", now - TimeDelta::days(400)).await;
        let overdue = stored_status(&replica, "taxation").await;
        assert!(!try_claim_scheduled(&mut replica, "taxation", now).await.unwrap());
        let waiting = stored_status(&replica, "taxation").await;
        assert_eq!(waiting.started_at, manual.started_at);
        assert_eq!(waiting.from_date, Some(from));
        assert_eq!(waiting.until_date, Some(until));
        assert_eq!(waiting.next_refresh_at, overdue.next_refresh_at);
        finish(owner, "taxation", Some("Taxation recalculation failed.")).await.unwrap();
        unlock(owner, "taxation").await.unwrap();
        let finished = stored_status(&replica, "taxation").await;
        assert_eq!(finished.from_date, Some(from));
        assert_eq!(finished.until_date, Some(until));
        assert_eq!(finished.next_refresh_at, overdue.next_refresh_at);

        // Two API replicas race one overdue slot. Both use the manual source
        // lock, and the winner starts full history regardless of the last range.
        let (a, b) = tokio::join!(
            try_claim_scheduled(owner, "taxation", now),
            try_claim_scheduled(&mut replica, "taxation", now),
        );
        let a = a.unwrap();
        assert_ne!(a, b.unwrap());
        let scheduled = stored_status(owner, "taxation").await;
        assert_eq!(scheduled.status, "running");
        assert_eq!(scheduled.started_at, Some(now));
        assert!(scheduled.from_date.is_none());
        assert!(scheduled.until_date.is_none());
        let next = Some(now + TimeDelta::weeks(1));
        assert_eq!(scheduled.next_refresh_at, next);
        let loser = if a { &mut replica } else { &mut *owner };
        assert!(!try_claim_scheduled(loser, "taxation", now).await.unwrap());
        let (claimed, coalesced) = try_claim(loser, "taxation", Some(from), Some(until)).await.unwrap();
        assert!(!claimed);
        assert!(coalesced.from_date.is_none());
        assert!(coalesced.until_date.is_none());
        assert_eq!(coalesced.next_refresh_at, next);
        let winner = if a { &mut *owner } else { &mut replica };
        finish(winner, "taxation", Some("Taxation recalculation failed.")).await.unwrap();
        unlock(winner, "taxation").await.unwrap();
        // A failed slot retains its cadence, rather than retrying on every tick.
        assert!(!try_claim_scheduled(owner, "taxation", now).await.unwrap());
        let failed = stored_status(&replica, "taxation").await;
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.next_refresh_at, next);
        assert_eq!(failed.last_success_at, scheduled.last_success_at);
        assert!(try_claim_scheduled(owner, "taxation", next.unwrap()).await.unwrap());
        finish(owner, "taxation", None).await.unwrap();
        unlock(owner, "taxation").await.unwrap();

        // A manual calculation at a future scheduled deadline leaves it intact.
        let before = stored_status(&replica, "taxation").await;
        let (claimed, manual) = try_claim(owner, "taxation", Some(from), Some(until)).await.unwrap();
        assert!(claimed);
        assert_eq!(manual.next_refresh_at, before.next_refresh_at);
        finish(owner, "taxation", None).await.unwrap();
        unlock(owner, "taxation").await.unwrap();
        assert_eq!(stored_status(&replica, "taxation").await.next_refresh_at, before.next_refresh_at);

        // Hold the configuration row before a stale scheduler claim arrives.
        // No sleeps: the claim must wait for the transaction's committed pause.
        make_due(owner, "taxation", now - TimeDelta::days(1)).await;
        let pause = replica.transaction().await.unwrap();
        pause.execute(
            "UPDATE calculation_status SET schedule_enabled = FALSE, next_refresh_at = NULL
             WHERE source = 'taxation'", &[],
        ).await.unwrap();
        let (claim, saved) = tokio::join!(
            try_claim_scheduled(owner, "taxation", now),
            pause.commit(),
        );
        saved.unwrap();
        assert!(!claim.unwrap());
        let paused = stored_status(owner, "taxation").await;
        assert!(!paused.schedule_enabled);
        assert!(paused.next_refresh_at.is_none());
        let paused = update_schedule(&replica, "taxation", false, "daily").await.unwrap();
        assert_eq!(paused.schedule_frequency, "daily");
        assert!(paused.next_refresh_at.is_none());

        update_schedule(&replica, "taxation", true, "daily").await.unwrap();
        make_due(owner, "taxation", now - TimeDelta::days(1)).await;
        let cadence = replica.transaction().await.unwrap();
        cadence.execute(
            "UPDATE calculation_status SET schedule_frequency = 'weekly'
             WHERE source = 'taxation'", &[],
        ).await.unwrap();
        let (claim, saved) = tokio::join!(
            try_claim_scheduled(owner, "taxation", now),
            cadence.commit(),
        );
        saved.unwrap();
        assert!(claim.unwrap());
        assert_eq!(stored_status(&replica, "taxation").await.next_refresh_at, next);
        // Pausing active work only clears its deadline, never its status/range.
        let paused = update_schedule(&replica, "taxation", false, "weekly").await.unwrap();
        assert_eq!(paused.status, "running");
        assert_eq!(paused.started_at, Some(now));
        assert!(paused.next_refresh_at.is_none());
        assert!(paused.from_date.is_none());
        assert!(paused.until_date.is_none());
        finish(owner, "taxation", None).await.unwrap();
        unlock(owner, "taxation").await.unwrap();

        // UTC intervals must ignore session DST; monthly uses calendar clamping,
        // including leap years, year rollover, and sub-second preservation.
        owner.batch_execute("SET TIME ZONE 'America/New_York'").await.unwrap();
        for (frequency, from, to) in [
            ("monthly", "2024-01-31T23:30:00.123456Z", "2024-02-29T23:30:00.123456Z"),
            ("monthly", "2025-01-31T23:30:00Z", "2025-02-28T23:30:00Z"),
            ("monthly", "2024-02-29T23:30:00Z", "2024-03-29T23:30:00Z"),
            ("monthly", "2024-03-31T23:30:00Z", "2024-04-30T23:30:00Z"),
            ("monthly", "2024-12-31T23:30:00Z", "2025-01-31T23:30:00Z"),
            ("daily", "2024-03-09T12:30:00Z", "2024-03-10T12:30:00Z"),
            ("weekly", "2024-03-09T12:30:00Z", "2024-03-16T12:30:00Z"),
        ] {
            let from: DateTime<Utc> = from.parse().unwrap();
            let to: DateTime<Utc> = to.parse().unwrap();
            update_schedule(&replica, "performance", true, frequency).await.unwrap();
            make_due(owner, "performance", from - TimeDelta::days(400)).await;
            assert!(try_claim_scheduled(owner, "performance", from).await.unwrap());
            let scheduled = stored_status(&replica, "performance").await;
            assert_eq!(scheduled.next_refresh_at, Some(to));
            assert!(scheduled.from_date.is_none());
            assert!(scheduled.until_date.is_none());
            finish(owner, "performance", None).await.unwrap();
            unlock(owner, "performance").await.unwrap();
            let identical = update_schedule(&replica, "performance", true, frequency).await.unwrap();
            assert_eq!(identical.next_refresh_at, Some(to));
        }

        // Losing a scheduled worker's actual session preserves its consumed
        // deadline and configuration while recovery marks only the run failed.
        make_due(owner, "performance", now - TimeDelta::days(1)).await;
        assert!(try_claim_scheduled(&mut replica, "performance", now).await.unwrap());
        let scheduled = stored_status(owner, "performance").await;
        drop(replica);
        replica_connection.await.unwrap().unwrap();
        let recovered = recover_interrupted(owner, "performance").await.unwrap();
        assert_eq!(recovered.status, "failed");
        assert_eq!(recovered.last_error.as_deref(), Some(INTERRUPTED_ERROR));
        assert_eq!(recovered.next_refresh_at, scheduled.next_refresh_at);
        assert_eq!(recovered.schedule_frequency, scheduled.schedule_frequency);
        assert!(recovered.schedule_enabled);
        assert_eq!(recovered.last_success_at, scheduled.last_success_at);
    }
}
