use chrono::{DateTime, Utc};
use tokio_postgres::Client;

// Separate from the pricing queue's advisory-lock namespace. Session locks are held
// on a dedicated connection for the entire import, including upstream requests.
const LOCK_NAMESPACE: i32 = 0x4442494d;

#[derive(Debug, PartialEq, Eq)]
pub struct DataImportSchedule {
    pub enabled: bool,
    pub frequency: String,
    pub next_refresh_at: Option<DateTime<Utc>>,
}

pub async fn read_schedule(db: &Client, source: &str) -> anyhow::Result<DataImportSchedule> {
    let row = db
        .query_one(
            "SELECT schedule_enabled, schedule_frequency, next_refresh_at
             FROM data_import_status WHERE source = $1",
            &[&source],
        )
        .await?;
    Ok(DataImportSchedule {
        enabled: row.get(0),
        frequency: row.get(1),
        next_refresh_at: row.get(2),
    })
}

/// One row update serializes saves with scheduled starts. Identical saves retain
/// the existing deadline; manual imports never touch these columns.
pub async fn update_schedule(
    db: &Client,
    source: &str,
    enabled: bool,
    frequency: &str,
) -> anyhow::Result<()> {
    source_key(source)?;
    anyhow::ensure!(
        matches!(frequency, "daily" | "weekly" | "monthly"),
        "Unsupported data import frequency"
    );
    db.execute(
        "UPDATE data_import_status SET schedule_enabled = $2, schedule_frequency = $3,
         next_refresh_at = CASE
             WHEN NOT $2 THEN NULL
             WHEN NOT schedule_enabled OR schedule_frequency <> $3 THEN clock_timestamp()
             ELSE next_refresh_at END
         WHERE source = $1",
        &[&source, &enabled, &frequency],
    )
    .await?;
    Ok(())
}

/// Caller owns this source's dedicated session lock until the worker finishes.
/// Recheck the persisted configuration while claiming: a stale tick cannot undo
/// a pause/cadence save or replay a slot already claimed by another API replica.
pub async fn start_scheduled(
    db: &Client,
    source: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let changed = db
        .execute(
            "UPDATE data_import_status
             SET status = 'running', started_at = $2,
                 finished_at = NULL, last_error = NULL,
                 next_refresh_at = (
                     ($2::timestamptz AT TIME ZONE 'UTC') +
                     CASE schedule_frequency
                         WHEN 'daily' THEN interval '1 day'
                         WHEN 'weekly' THEN interval '1 week'
                         WHEN 'monthly' THEN interval '1 month'
                     END
                 ) AT TIME ZONE 'UTC'
             WHERE source = $1 AND schedule_enabled
                 AND next_refresh_at <= $2",
            &[&source, &now],
        )
        .await?;
    // Missed intervals coalesce into this one execution, advancing from now in
    // UTC (including month-end clamping), not from the old overdue deadline.
    Ok(changed != 0)
}

pub async fn try_lock(db: &Client, source: &str) -> anyhow::Result<bool> {
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

fn source_key(source: &str) -> anyhow::Result<i32> {
    match source {
        "polygon" => Ok(1),
        "ecb" => Ok(2),
        _ => anyhow::bail!("Unsupported data import source"),
    }
}

// Recovery takes the same lock as refresh, so it cannot mistake a live import
// (including one running in another API process) for an interrupted import.
pub async fn recover_interrupted(db: &Client, source: &str) -> anyhow::Result<()> {
    if try_lock(db, source).await? {
        let result = db
            .execute(
                "UPDATE data_import_status SET status = 'failed', finished_at = now(),
             last_error = 'Import interrupted before completion. Import again to retry.'
             WHERE source = $1 AND status = 'running'",
                &[&source],
            )
            .await;
        unlock(db, source).await?;
        result?;
    }
    Ok(())
}

// Caller must own this source's session lock until finish completes.
pub async fn start(db: &Client, source: &str) -> anyhow::Result<()> {
    db.execute(
        "UPDATE data_import_status SET status = 'running', started_at = now(),
         finished_at = NULL, last_error = NULL WHERE source = $1",
        &[&source],
    )
    .await?;
    Ok(())
}

pub async fn finish(db: &Client, source: &str, error: Option<&str>) -> anyhow::Result<()> {
    db.execute(
        "UPDATE data_import_status SET status = CASE WHEN $2::text IS NULL THEN 'succeeded' ELSE 'failed' END,
         finished_at = now(), last_error = $2,
         last_success_at = CASE WHEN $2::text IS NULL THEN now() ELSE last_success_at END
         WHERE source = $1",
        &[&source, &error],
    ).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeDelta, Timelike, Utc};
    use tokio_postgres::NoTls;

    #[tokio::test]
    #[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
    async fn locks_coalesce_refreshes_and_recover_interrupted_imports() {
        let url = std::env::var("TEST_POSTGRES_URL").expect("Set an isolated TEST_POSTGRES_URL");
        let (mut first, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        let first_connection = tokio::spawn(connection);
        let schema = format!(
            "imports_test_{}_{}",
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
        let (second, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
        tokio::spawn(async move {
            connection.await.unwrap();
        });
        second
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .unwrap();

        assert!(try_lock(&first, "polygon").await.unwrap());
        start(&first, "polygon").await.unwrap();
        assert!(!try_lock(&second, "polygon").await.unwrap());
        assert!(try_lock(&second, "ecb").await.unwrap());
        unlock(&second, "ecb").await.unwrap();
        recover_interrupted(&second, "polygon").await.unwrap();
        let status: String = second
            .query_one(
                "SELECT status FROM data_import_status WHERE source = 'polygon'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(status, "running");
        // Terminating the owning connection simulates process death, not a clean unlock.
        drop(first);
        first_connection.await.unwrap().unwrap();
        recover_interrupted(&second, "polygon").await.unwrap();
        let row = second.query_one("SELECT status, finished_at, last_error FROM data_import_status WHERE source = 'polygon'", &[]).await.unwrap();
        assert_eq!(row.get::<_, String>(0), "failed");
        assert!(row.get::<_, Option<DateTime<Utc>>>(1).is_some());
        assert!(row
            .get::<_, Option<String>>(2)
            .unwrap()
            .contains("interrupted"));

        assert!(try_lock(&second, "polygon").await.unwrap());
        start(&second, "polygon").await.unwrap();
        finish(&second, "polygon", None).await.unwrap();
        let last_success: DateTime<Utc> = second
            .query_one(
                "SELECT last_success_at FROM data_import_status WHERE source = 'polygon'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        start(&second, "polygon").await.unwrap();
        finish(&second, "polygon", Some("upstream unavailable"))
            .await
            .unwrap();
        let row = second
            .query_one(
                "SELECT status, last_success_at FROM data_import_status WHERE source = 'polygon'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "failed");
        assert_eq!(row.get::<_, DateTime<Utc>>(1), last_success);
        unlock(&second, "polygon").await.unwrap();
        // Keep the advisory-lock scenarios in one test: those locks are scoped
        // to the database, not the otherwise isolated per-test schemas.
        exercise_schedules(&second, &url, &schema).await;
        second
            .batch_execute(&format!(
                "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
            ))
            .await
            .unwrap();
    }

    async fn exercise_schedules(owner: &Client, url: &str, schema: &str) {
        let (mut replica, connection) = tokio_postgres::connect(url, NoTls).await.unwrap();
        tokio::spawn(async move {
            connection.await.unwrap();
        });
        replica
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .unwrap();

        for (source, frequency) in [("polygon", "monthly"), ("ecb", "daily")] {
            assert_eq!(
                read_schedule(owner, source).await.unwrap(),
                DataImportSchedule {
                    enabled: false,
                    frequency: frequency.to_owned(),
                    next_refresh_at: None,
                }
            );
        }
        update_schedule(owner, "polygon", true, "monthly").await.unwrap();
        let enabled = read_schedule(&replica, "polygon").await.unwrap();
        assert!(enabled.enabled);
        assert!(enabled.next_refresh_at.unwrap() <= Utc::now());

        // A different database connection reads persisted configuration. Saving
        // it unchanged must preserve even an overdue or future deadline.
        update_schedule(&replica, "polygon", true, "monthly").await.unwrap();
        assert_eq!(read_schedule(owner, "polygon").await.unwrap(), enabled);
        let future: DateTime<Utc> = "2100-01-01T00:00:00Z".parse().unwrap();
        owner
            .execute(
                "UPDATE data_import_status SET next_refresh_at = $1 WHERE source = 'polygon'",
                &[&future],
            )
            .await
            .unwrap();
        update_schedule(&replica, "polygon", true, "monthly").await.unwrap();
        assert_eq!(read_schedule(owner, "polygon").await.unwrap().next_refresh_at, Some(future));
        update_schedule(&replica, "polygon", true, "weekly").await.unwrap();
        let changed = read_schedule(owner, "polygon").await.unwrap();
        assert_eq!(changed.frequency, "weekly");
        assert!(changed.next_refresh_at.unwrap() <= Utc::now());
        assert_ne!(changed.next_refresh_at, Some(future));

        assert!(update_schedule(&replica, "polygon", true, "hourly").await.is_err());
        assert!(update_schedule(&replica, "unknown", true, "daily").await.is_err());
        assert_eq!(read_schedule(owner, "polygon").await.unwrap(), changed);
        update_schedule(&replica, "polygon", false, "weekly").await.unwrap();
        let paused = read_schedule(owner, "polygon").await.unwrap();
        assert!(!paused.enabled);
        assert_eq!(paused.next_refresh_at, None);
        update_schedule(&replica, "polygon", false, "daily").await.unwrap();
        let paused = read_schedule(owner, "polygon").await.unwrap();
        assert_eq!(paused.frequency, "daily");
        assert_eq!(paused.next_refresh_at, None);

        assert!(try_lock(owner, "polygon").await.unwrap());
        // Manual imports work while paused and leave the configuration intact.
        start(owner, "polygon").await.unwrap();
        finish(owner, "polygon", None).await.unwrap();
        assert_eq!(read_schedule(&replica, "polygon").await.unwrap(), paused);
        update_schedule(&replica, "polygon", true, "daily").await.unwrap();
        let resumed = read_schedule(owner, "polygon").await.unwrap();
        assert!(resumed.next_refresh_at.unwrap() <= Utc::now());
        start(owner, "polygon").await.unwrap();
        finish(owner, "polygon", None).await.unwrap();
        assert_eq!(read_schedule(&replica, "polygon").await.unwrap(), resumed);

        let now = Utc::now().with_nanosecond(0).unwrap();
        make_due(owner, now - TimeDelta::days(400)).await;
        let overdue = read_schedule(&replica, "polygon").await.unwrap();
        // A losing replica (or manual request) cannot consume a due slot.
        assert!(!try_lock(&replica, "polygon").await.unwrap());
        assert_eq!(read_schedule(&replica, "polygon").await.unwrap(), overdue);
        assert!(start_scheduled(owner, "polygon", now).await.unwrap());
        assert_eq!(
            read_schedule(&replica, "polygon").await.unwrap().next_refresh_at,
            Some(now + TimeDelta::days(1))
        );
        let running: String = replica
            .query_one("SELECT status FROM data_import_status WHERE source = 'polygon'", &[])
            .await.unwrap().get(0);
        assert_eq!(running, "running");
        let advanced = read_schedule(&replica, "polygon").await.unwrap();
        update_schedule(&replica, "polygon", true, "daily").await.unwrap();
        assert_eq!(read_schedule(owner, "polygon").await.unwrap(), advanced);
        finish(owner, "polygon", None).await.unwrap();
        unlock(owner, "polygon").await.unwrap();
        assert!(try_lock(&replica, "polygon").await.unwrap());
        // Rechecking after the winner releases its lock must not run twice.
        assert!(!start_scheduled(&replica, "polygon", now).await.unwrap());
        unlock(&replica, "polygon").await.unwrap();

        assert!(try_lock(owner, "polygon").await.unwrap());
        make_due(owner, now - TimeDelta::days(1)).await;
        // This pause holds the row lock before the stale tick's claim arrives.
        // Claiming must recheck after the pause commits, not use an earlier read.
        let pause = replica.transaction().await.unwrap();
        pause.execute(
            "UPDATE data_import_status SET schedule_enabled = FALSE, next_refresh_at = NULL
             WHERE source = 'polygon'", &[],
        ).await.unwrap();
        let (claim, saved) = tokio::join!(
            start_scheduled(owner, "polygon", now),
            pause.commit(),
        );
        saved.unwrap();
        assert!(!claim.unwrap());
        assert_eq!(read_schedule(owner, "polygon").await.unwrap().next_refresh_at, None);

        update_schedule(&replica, "polygon", true, "daily").await.unwrap();
        make_due(owner, now - TimeDelta::days(1)).await;
        let cadence = replica.transaction().await.unwrap();
        cadence.execute(
            "UPDATE data_import_status SET schedule_frequency = 'weekly'
             WHERE source = 'polygon'", &[],
        ).await.unwrap();
        let (claim, saved) = tokio::join!(
            start_scheduled(owner, "polygon", now),
            cadence.commit(),
        );
        saved.unwrap();
        assert!(claim.unwrap());
        assert_eq!(
            read_schedule(&replica, "polygon").await.unwrap().next_refresh_at,
            Some(now + TimeDelta::weeks(1))
        );
        // Pausing an already claimed import cannot cancel or overwrite its state.
        update_schedule(&replica, "polygon", false, "weekly").await.unwrap();
        let row = replica.query_one(
            "SELECT status, next_refresh_at FROM data_import_status WHERE source = 'polygon'", &[],
        ).await.unwrap();
        assert_eq!(row.get::<_, String>(0), "running");
        assert_eq!(row.get::<_, Option<DateTime<Utc>>>(1), None);
        finish(owner, "polygon", None).await.unwrap();

        // PostgreSQL session time zones and DST must not change the public UTC
        // cadence; monthly means a calendar month, clamping at the destination end.
        owner.batch_execute("SET TIME ZONE 'America/New_York'").await.unwrap();
        for (frequency, from, to) in [
            ("monthly", "2024-01-31T23:30:00Z", "2024-02-29T23:30:00Z"),
            ("monthly", "2025-01-31T23:30:00Z", "2025-02-28T23:30:00Z"),
            ("monthly", "2024-03-31T23:30:00Z", "2024-04-30T23:30:00Z"),
            ("monthly", "2024-12-31T23:30:00Z", "2025-01-31T23:30:00Z"),
            ("daily", "2024-03-09T12:30:00Z", "2024-03-10T12:30:00Z"),
            ("weekly", "2024-03-09T12:30:00Z", "2024-03-16T12:30:00Z"),
        ] {
            let from: DateTime<Utc> = from.parse().unwrap();
            let to: DateTime<Utc> = to.parse().unwrap();
            update_schedule(&replica, "polygon", true, frequency).await.unwrap();
            make_due(owner, from - TimeDelta::seconds(1)).await;
            assert!(start_scheduled(owner, "polygon", from).await.unwrap());
            assert_eq!(
                read_schedule(&replica, "polygon").await.unwrap().next_refresh_at,
                Some(to)
            );
            finish(owner, "polygon", None).await.unwrap();
        }
        unlock(owner, "polygon").await.unwrap();
    }

    async fn make_due(db: &Client, deadline: DateTime<Utc>) {
        db.execute(
            "UPDATE data_import_status SET next_refresh_at = $1 WHERE source = 'polygon'",
            &[&deadline],
        )
        .await
        .unwrap();
    }
}
