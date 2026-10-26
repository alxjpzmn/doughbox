use super::*;
use crate::database::queries::price_scheduler as scheduler;

async fn snapshot(db: &Client, table: &str) -> Vec<String> {
    db.query(
        &format!("SELECT to_jsonb(t)::text FROM {table} t ORDER BY to_jsonb(t)::text"),
        &[],
    )
    .await
    .unwrap()
    .into_iter()
    .map(|row| row.get(0))
    .collect()
}

async fn assert_config(db: &Client, enabled: bool, interval: u32, feeds: &[Feed]) {
    let actual = scheduler::read(db).await.unwrap();
    assert_eq!(actual.enabled, enabled);
    assert_eq!(actual.interval_minutes, interval);
    let mut actual_feeds: Vec<_> = actual.feeds.iter().map(|feed| feed.key()).collect();
    let mut expected_feeds: Vec<_> = feeds.iter().map(|feed| feed.key()).collect();
    actual_feeds.sort_unstable();
    expected_feeds.sort_unstable();
    assert_eq!(actual_feeds, expected_feeds);
}

async fn assert_fresh_job(db: &Client, id: i64, feed: Feed) {
    let row = db
        .query_one(
            "SELECT feed, status, attempts, lease_until IS NULL AS unleased,
                NOT rerun_requested AS no_rerun, last_error IS NULL AS no_error,
                finished_at IS NULL AS unfinished, available_at <= clock_timestamp() AS due,
                files_processed, reports_processed
             FROM background_job WHERE id = $1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>("feed"), feed.key());
    assert_eq!(row.get::<_, String>("status"), "queued");
    assert_eq!(row.get::<_, i32>("attempts"), 0);
    assert!(row.get::<_, bool>("unleased"));
    assert!(row.get::<_, bool>("no_rerun"));
    assert!(row.get::<_, bool>("no_error"));
    assert!(row.get::<_, bool>("unfinished"));
    assert!(row.get::<_, bool>("due"));
    assert_eq!(row.get::<_, i32>("files_processed"), 0);
    assert_eq!(row.get::<_, i32>("reports_processed"), 0);
}

async fn assert_retired_job(db: &Client, id: i64) {
    let row = db
        .query_one(
            "SELECT status, finished_at IS NOT NULL AS finished,
                lease_until IS NULL AS unleased, rerun_requested
             FROM background_job WHERE id = $1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>("status"), "failed");
    assert!(row.get::<_, bool>("finished"));
    assert!(row.get::<_, bool>("unleased"));
    assert!(!row.get::<_, bool>("rerun_requested"));
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_scheduler_bootstrap_and_restart_preserve_persisted_choices_and_work() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let completed = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    price::commit_file(
        &mut db,
        &completed,
        "before-bootstrap",
        "universe",
        "saved-hash",
        None,
        vec![],
    )
    .await
    .unwrap();
    jobs::finish(&mut db, &completed).await.unwrap();
    let jobs_before = snapshot(&db, "background_job").await;
    let checkpoints_before = snapshot(&db, "market_data_file").await;
    let last_success: chrono::DateTime<Utc> = db
        .query_one(
            "SELECT last_success_at FROM price_feed_schedule WHERE feed = $1",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap()
        .get(0);
    let defaults = PriceConfig {
        enabled: false,
        interval_minutes: 11,
        workers: 2,
        feeds: vec![Feed::Xetra, Feed::Euronext],
    };
    configure_scheduler(&mut db, &defaults).await.unwrap();
    assert_config(&db, false, 11, &[Feed::Xetra, Feed::Euronext]).await;
    assert_eq!(snapshot(&db, "background_job").await, jobs_before);
    assert_eq!(snapshot(&db, "market_data_file").await, checkpoints_before);
    let preserved_success: chrono::DateTime<Utc> = db
        .query_one(
            "SELECT last_success_at FROM price_feed_schedule WHERE feed = $1",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(preserved_success, last_success);
    let mut existing_feeds: Vec<String> = db
        .query("SELECT feed FROM price_feed_schedule ORDER BY feed", &[])
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    let mut all_feeds: Vec<String> = Feed::ALL.iter().map(|feed| feed.key().into()).collect();
    existing_feeds.sort_unstable();
    all_feeds.sort_unstable();
    assert_eq!(existing_feeds, all_feeds);

    scheduler::update(&mut db, true, 43, 3, &[Feed::Frankfurt, Feed::Cboe])
        .await
        .unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    let settings_before = snapshot(&db, "price_scheduler_settings").await;
    let schedules_before = snapshot(&db, "price_feed_schedule").await;
    let jobs_before = snapshot(&db, "background_job").await;
    // A new connection simulates a restart without replacing saved UI choices.
    let url = std::env::var("TEST_POSTGRES_URL").unwrap();
    let (mut restarted, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move { connection.await.unwrap() });
    restarted
        .batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    scheduler::initialize(&mut restarted).await.unwrap();
    assert_config(&restarted, true, 43, &[Feed::Frankfurt, Feed::Cboe]).await;
    assert_eq!(scheduler::read(&restarted).await.unwrap().workers, 3);
    assert_eq!(
        snapshot(&restarted, "price_scheduler_settings").await,
        settings_before
    );
    assert_eq!(
        snapshot(&restarted, "price_feed_schedule").await,
        schedules_before
    );
    assert_eq!(snapshot(&restarted, "background_job").await, jobs_before);
    assert_eq!(
        snapshot(&restarted, "market_data_file").await,
        checkpoints_before
    );
    scheduler::schedule_due(&mut restarted).await.unwrap();
    assert_eq!(snapshot(&restarted, "background_job").await, jobs_before);
    drop(restarted);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_scheduler_pause_selection_interval_and_coalescing_control_work() {
    let (mut db, schema) = test_database().await;
    let defaults = PriceConfig {
        enabled: false,
        interval_minutes: 7,
        feeds: vec![Feed::Xetra],
        ..config()
    };
    configure_scheduler(&mut db, &defaults).await.unwrap();
    db.execute(
        "UPDATE price_feed_schedule SET next_refresh_at = now() - interval '6 hours'",
        &[],
    )
    .await
    .unwrap();
    let paused_schedules = snapshot(&db, "price_feed_schedule").await;
    scheduler::schedule_due(&mut db).await.unwrap();
    assert!(snapshot(&db, "background_job").await.is_empty());
    assert_eq!(snapshot(&db, "price_feed_schedule").await, paused_schedules);
    assert_config(&db, false, 7, &[Feed::Xetra]).await;
    db.execute(
        "UPDATE price_feed_schedule SET next_refresh_at = now() + interval '1 day' WHERE feed = $1",
        &[&Feed::Xetra.key()],
    )
    .await
    .unwrap();

    scheduler::update(&mut db, true, 7, 2, &[Feed::Xetra])
        .await
        .unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    let queued = snapshot(&db, "background_job").await;
    assert_eq!(queued.len(), 1);
    let next_seconds: f64 = db
        .query_one(
            "SELECT EXTRACT(EPOCH FROM next_refresh_at - clock_timestamp())::float8
             FROM price_feed_schedule WHERE feed = $1",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap()
        .get(0);
    assert!((410.0..=420.0).contains(&next_seconds));
    assert!(jobs::claim(&db, &[Feed::Tradegate])
        .await
        .unwrap()
        .is_none());
    let running = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    db.execute(
        "UPDATE price_feed_schedule SET next_refresh_at = now() - interval '6 hours'",
        &[],
    )
    .await
    .unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    assert_eq!(snapshot(&db, "background_job").await.len(), 1);
    jobs::finish(&mut db, &running).await.unwrap();
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());

    let schedules_before = snapshot(&db, "price_feed_schedule").await;
    scheduler::update(&mut db, true, 7, 2, &[Feed::Xetra])
        .await
        .unwrap();
    assert_eq!(snapshot(&db, "price_feed_schedule").await, schedules_before);
    scheduler::update(&mut db, true, 13, 2, &[Feed::Tradegate, Feed::Xetra])
        .await
        .unwrap();
    let rows = db
        .query(
            "SELECT feed, EXTRACT(EPOCH FROM next_refresh_at - clock_timestamp())::float8 AS seconds
             FROM price_feed_schedule WHERE enabled ORDER BY feed",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let seconds: f64 = row.get("seconds");
        if row.get::<_, String>("feed") == Feed::Xetra.key() {
            assert!((770.0..=780.0).contains(&seconds));
        } else {
            assert!(
                seconds <= 0.0,
                "a newly selected feed must be immediately due"
            );
        }
    }
    scheduler::schedule_due(&mut db).await.unwrap();
    let persisted_interval_seconds: f64 = db
        .query_one(
            "SELECT EXTRACT(EPOCH FROM next_refresh_at - clock_timestamp())::float8
             FROM price_feed_schedule WHERE feed = $1",
            &[&Feed::Tradegate.key()],
        )
        .await
        .unwrap()
        .get(0);
    assert!((770.0..=780.0).contains(&persisted_interval_seconds));
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    let tradegate = jobs::claim(&db, &[Feed::Tradegate]).await.unwrap().unwrap();
    jobs::finish(&mut db, &tradegate).await.unwrap();

    scheduler::update(&mut db, true, 13, 2, &[Feed::Tradegate])
        .await
        .unwrap();
    db.execute(
        "UPDATE price_feed_schedule SET next_refresh_at = now() - interval '6 hours'",
        &[],
    )
    .await
    .unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    let selected_job = jobs::claim(&db, &[Feed::Tradegate]).await.unwrap().unwrap();
    jobs::finish(&mut db, &selected_job).await.unwrap();
    scheduler::update(&mut db, false, 13, 2, &[Feed::Tradegate])
        .await
        .unwrap();
    db.execute(
        "UPDATE price_feed_schedule SET next_refresh_at = now() - interval '6 hours'",
        &[],
    )
    .await
    .unwrap();
    let paused_jobs = snapshot(&db, "background_job").await;
    let paused_schedules = snapshot(&db, "price_feed_schedule").await;
    scheduler::schedule_due(&mut db).await.unwrap();
    assert_eq!(snapshot(&db, "background_job").await, paused_jobs);
    assert_eq!(snapshot(&db, "price_feed_schedule").await, paused_schedules);
    assert_config(&db, false, 13, &[Feed::Tradegate]).await;
    scheduler::update(&mut db, false, 13, 2, &[]).await.unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    assert_config(&db, false, 13, &[]).await;
    assert_eq!(snapshot(&db, "background_job").await, paused_jobs);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_scheduler_invalid_updates_are_atomic_and_leave_due_work_unchanged() {
    let (mut db, schema) = test_database().await;
    configure_scheduler(&mut db, &config()).await.unwrap();
    scheduler::schedule_due(&mut db).await.unwrap();
    let settings_before = snapshot(&db, "price_scheduler_settings").await;
    let schedules_before = snapshot(&db, "price_feed_schedule").await;
    let jobs_before = snapshot(&db, "background_job").await;
    let invalid = [
        (false, 0, vec![Feed::Frankfurt]),
        (true, 1441, vec![Feed::Cboe]),
        (true, u32::MAX, vec![Feed::Euronext]),
        (true, 17, vec![]),
    ];
    for (enabled, interval, feeds) in invalid {
        assert!(scheduler::update(&mut db, enabled, interval, 2, &feeds)
            .await
            .is_err());
        assert_config(&db, true, 30, &[Feed::Xetra, Feed::Tradegate]).await;
        assert_eq!(
            snapshot(&db, "price_scheduler_settings").await,
            settings_before
        );
        assert_eq!(snapshot(&db, "price_feed_schedule").await, schedules_before);
        assert_eq!(snapshot(&db, "background_job").await, jobs_before);
        scheduler::schedule_due(&mut db).await.unwrap();
        assert_eq!(snapshot(&db, "background_job").await, jobs_before);
    }
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_scheduler_reset_replaces_queued_backoff_and_failed_jobs_without_losing_history(
) {
    let (mut db, schema) = test_database().await;
    configure_scheduler(&mut db, &config()).await.unwrap();
    let queued = jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap()[0];
    let first_reset = scheduler::reset(&mut db, Feed::Xetra).await.unwrap();
    assert_ne!(first_reset, queued);
    assert_retired_job(&db, queued).await;
    assert_fresh_job(&db, first_reset, Feed::Xetra).await;
    let worker = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(worker.id, first_reset);
    jobs::fail(&mut db, &worker, "provider backoff", 3600)
        .await
        .unwrap();
    assert_eq!(
        jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap(),
        [worker.id]
    );
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    let retry_reset = scheduler::reset(&mut db, Feed::Xetra).await.unwrap();
    assert_ne!(retry_reset, worker.id);
    assert_retired_job(&db, worker.id).await;
    assert_fresh_job(&db, retry_reset, Feed::Xetra).await;
    let mut last_attempt = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(last_attempt.id, retry_reset);
    assert_eq!(last_attempt.attempts, 1);
    last_attempt.attempts = 4;
    db.execute(
        "UPDATE background_job SET attempts = $2 WHERE id = $1",
        &[&last_attempt.id, &last_attempt.attempts],
    )
    .await
    .unwrap();
    jobs::fail(&mut db, &last_attempt, "exhausted retries", 3600)
        .await
        .unwrap();
    let history_before = snapshot(&db, "background_job").await;
    let failed_reset = scheduler::reset(&mut db, Feed::Xetra).await.unwrap();
    assert_ne!(failed_reset, last_attempt.id);
    assert_fresh_job(&db, failed_reset, Feed::Xetra).await;
    let history_after = snapshot(&db, "background_job").await;
    assert_eq!(history_after.len(), history_before.len() + 1);
    for row in history_before {
        assert!(
            history_after.contains(&row),
            "reset changed terminal job history"
        );
    }
    let feed_error: Option<String> = db
        .query_one(
            "SELECT last_error FROM price_feed_schedule WHERE feed = $1",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap()
        .get(0);
    assert!(feed_error.is_none());
    let fresh = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(fresh.id, failed_reset);
    assert_eq!(fresh.attempts, 1);
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_scheduler_reset_fences_live_worker_and_preserves_prices_progress_and_other_feeds(
) {
    let (mut db, schema) = test_database().await;
    configure_scheduler(&mut db, &config()).await.unwrap();
    db.batch_execute(
        "INSERT INTO instrument(id, name, price, last_price_update, fallback_price, fallback_at)
         VALUES ('DE0007164600', 'Selected equity', 50, '2020-01-01', 50, '2020-01-01'),
                ('DE0000000001', 'Pinned equity', 60, '2020-01-01', 60, '2020-01-01')",
    )
    .await
    .unwrap();
    jobs::enqueue(&mut db, &[Feed::Xetra, Feed::Tradegate])
        .await
        .unwrap();
    let completed = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    price::commit_file(
        &mut db,
        &completed,
        "completed-file",
        "universe",
        "completed-hash",
        Some("completed-etag"),
        vec![report("selected-before-reset", 10, dec!(101))],
    )
    .await
    .unwrap();
    jobs::finish(&mut db, &completed).await.unwrap();
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let running = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let mut pinned_report = report("pinned-before-reset", 5, dec!(202));
    pinned_report.isin = "DE0000000001".into();
    price::commit_file(
        &mut db,
        &running,
        "partial-file",
        "universe",
        "partial-hash",
        Some("partial-etag"),
        vec![pinned_report],
    )
    .await
    .unwrap();
    price::set_manual(&mut db, "DE0000000001", dec!(333), Utc::now())
        .await
        .unwrap();
    assert_eq!(selected(&db).await.0, dec!(101));
    assert_eq!(
        jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap(),
        [running.id]
    );
    let other_worker = jobs::claim(&db, &[Feed::Tradegate]).await.unwrap().unwrap();
    price::commit_file(
        &mut db,
        &other_worker,
        "other-feed-file",
        "universe",
        "other-hash",
        None,
        vec![],
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE price_feed_schedule SET last_error = 'old error' WHERE feed = $1",
        &[&Feed::Xetra.key()],
    )
    .await
    .unwrap();
    // Reset must not accidentally invoke the normal successful-job retention sweep.
    db.execute(
        "UPDATE background_job SET finished_at = now() - interval '30 days' WHERE id = $1",
        &[&completed.id],
    )
    .await
    .unwrap();
    db.batch_execute(
        "UPDATE market_data_file SET processed_at = now() - interval '30 days';
         UPDATE market_price_report SET received_at = now() - interval '30 days'",
    )
    .await
    .unwrap();
    let prices_before = snapshot(&db, "instrument").await;
    let reports_before = snapshot(&db, "market_price_report").await;
    let checkpoints_before = snapshot(&db, "market_data_file").await;
    let settings_before = snapshot(&db, "price_scheduler_settings").await;
    let completed_before: String = db
        .query_one(
            "SELECT to_jsonb(j)::text FROM background_job j WHERE id = $1",
            &[&completed.id],
        )
        .await
        .unwrap()
        .get(0);
    let other_job_before: String = db
        .query_one(
            "SELECT to_jsonb(j)::text FROM background_job j WHERE id = $1",
            &[&other_worker.id],
        )
        .await
        .unwrap()
        .get(0);
    let other_schedules_before: Vec<String> = db
        .query(
            "SELECT to_jsonb(s)::text FROM price_feed_schedule s WHERE feed <> $1 ORDER BY feed",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    let schedule_before = db
        .query_one(
            "SELECT next_refresh_at, last_success_at FROM price_feed_schedule WHERE feed = $1",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap();
    let next_before: chrono::DateTime<Utc> = schedule_before.get("next_refresh_at");
    let success_before: chrono::DateTime<Utc> = schedule_before.get("last_success_at");

    let replacement = scheduler::reset(&mut db, Feed::Xetra).await.unwrap();
    assert_ne!(replacement, running.id);
    assert_retired_job(&db, running.id).await;
    assert_fresh_job(&db, replacement, Feed::Xetra).await;
    let retired_progress = db
        .query_one(
            "SELECT files_processed, reports_processed FROM background_job WHERE id = $1",
            &[&running.id],
        )
        .await
        .unwrap();
    assert_eq!(retired_progress.get::<_, i32>("files_processed"), 1);
    assert_eq!(retired_progress.get::<_, i32>("reports_processed"), 1);
    assert_eq!(snapshot(&db, "instrument").await, prices_before);
    assert_eq!(snapshot(&db, "market_price_report").await, reports_before);
    assert_eq!(snapshot(&db, "market_data_file").await, checkpoints_before);
    assert_eq!(
        snapshot(&db, "price_scheduler_settings").await,
        settings_before
    );
    let completed_after: String = db
        .query_one(
            "SELECT to_jsonb(j)::text FROM background_job j WHERE id = $1",
            &[&completed.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(completed_after, completed_before);
    let other_job_after: String = db
        .query_one(
            "SELECT to_jsonb(j)::text FROM background_job j WHERE id = $1",
            &[&other_worker.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(other_job_after, other_job_before);
    let other_schedules_after: Vec<String> = db
        .query(
            "SELECT to_jsonb(s)::text FROM price_feed_schedule s WHERE feed <> $1 ORDER BY feed",
            &[&Feed::Xetra.key()],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(other_schedules_after, other_schedules_before);
    let schedule_after = db
        .query_one("SELECT next_refresh_at, last_success_at, last_error FROM price_feed_schedule WHERE feed = $1", &[&Feed::Xetra.key()])
        .await
        .unwrap();
    assert_eq!(
        schedule_after.get::<_, chrono::DateTime<Utc>>("next_refresh_at"),
        next_before
    );
    assert_eq!(
        schedule_after.get::<_, chrono::DateTime<Utc>>("last_success_at"),
        success_before
    );
    assert!(schedule_after
        .get::<_, Option<String>>("last_error")
        .is_none());

    // The old lease was still valid: reset, not lease expiry, must fence every write path.
    let jobs_after_reset = snapshot(&db, "background_job").await;
    let schedules_after_reset = snapshot(&db, "price_feed_schedule").await;
    assert!(jobs::renew(&db, &running).await.is_err());
    assert!(jobs::release(&db, &running).await.is_err());
    assert!(jobs::finish(&mut db, &running).await.is_err());
    assert!(jobs::fail(&mut db, &running, "stale worker error", 1)
        .await
        .is_err());
    assert!(price::commit_file(
        &mut db,
        &running,
        "stale-worker-file",
        "universe",
        "stale-hash",
        None,
        vec![report("stale-worker-report", 0, dec!(999))],
    )
    .await
    .is_err());
    assert_eq!(snapshot(&db, "background_job").await, jobs_after_reset);
    assert_eq!(
        snapshot(&db, "price_feed_schedule").await,
        schedules_after_reset
    );
    assert_eq!(snapshot(&db, "instrument").await, prices_before);
    assert_eq!(snapshot(&db, "market_price_report").await, reports_before);
    assert_eq!(snapshot(&db, "market_data_file").await, checkpoints_before);
    let checkpoints = price::checkpoints(&db, Feed::Xetra.key(), "universe")
        .await
        .unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints["completed-file"].content_hash, "completed-hash");
    assert_eq!(checkpoints["partial-file"].content_hash, "partial-hash");
    let fresh = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(fresh.id, replacement);
    assert_eq!(fresh.attempts, 1);
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    jobs::renew(&db, &other_worker).await.unwrap();
    price::commit_file(
        &mut db,
        &fresh,
        "fresh-worker-file",
        "universe",
        "fresh-hash",
        None,
        vec![report("fresh-worker-report", 0, dec!(102))],
    )
    .await
    .unwrap();
    jobs::finish(&mut db, &fresh).await.unwrap();
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    assert_eq!(selected(&db).await.0, dec!(102));
    let pinned: (Decimal, bool, String) = {
        let row = db
            .query_one("SELECT price, price_pinned, price_source FROM instrument WHERE id = 'DE0000000001'", &[])
            .await
            .unwrap();
        (row.get(0), row.get(1), row.get(2))
    };
    assert_eq!(pinned, (dec!(333), true, "manual".into()));
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_ui_settings_upgrade_and_worker_changes_preserve_existing_schedules() {
    let (mut db, schema) = test_database_at(refinery::Target::Version(11)).await;
    db.batch_execute(
        "INSERT INTO price_scheduler_settings(singleton, enabled, interval_minutes) VALUES(TRUE, TRUE, 43);
         INSERT INTO price_feed_schedule(feed, enabled, next_refresh_at)
            VALUES('DFRA-posttrade', TRUE, now() + interval '1 day');
         INSERT INTO market_data_file(feed, file_key, universe_hash, content_hash)
            VALUES('DFRA-posttrade', 'saved-batch', 'universe', 'saved-hash');",
    ).await.unwrap();
    let original_schedule: String = db
        .query_one(
            "SELECT to_jsonb(s)::text FROM price_feed_schedule s WHERE feed = 'DFRA-posttrade'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let checkpoints = snapshot(&db, "market_data_file").await;
    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    scheduler::initialize(&mut db).await.unwrap();
    assert_config(&db, true, 43, &[Feed::Frankfurt]).await;
    assert_eq!(
        db.query_one(
            "SELECT to_jsonb(s)::text FROM price_feed_schedule s WHERE feed = 'DFRA-posttrade'",
            &[],
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        original_schedule
    );
    jobs::enqueue(&mut db, &[Feed::Frankfurt]).await.unwrap();
    let schedules = snapshot(&db, "price_feed_schedule").await;
    let jobs_before = snapshot(&db, "background_job").await;
    scheduler::update(&mut db, true, 43, 3, &[Feed::Frankfurt])
        .await
        .unwrap();
    scheduler::initialize(&mut db).await.unwrap();
    assert_eq!(scheduler::read(&db).await.unwrap().workers, 3);
    assert_config(&db, true, 43, &[Feed::Frankfurt]).await;
    assert_eq!(snapshot(&db, "price_feed_schedule").await, schedules);
    assert_eq!(snapshot(&db, "background_job").await, jobs_before);
    assert_eq!(snapshot(&db, "market_data_file").await, checkpoints);
    let settings = snapshot(&db, "price_scheduler_settings").await;
    for invalid_workers in [0, 5] {
        assert!(
            scheduler::update(&mut db, false, 1, invalid_workers, &[Feed::Cboe])
                .await
                .is_err()
        );
        assert_eq!(snapshot(&db, "price_scheduler_settings").await, settings);
        assert_eq!(snapshot(&db, "price_feed_schedule").await, schedules);
        assert_eq!(snapshot(&db, "background_job").await, jobs_before);
    }
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_log_settings_persist_and_reject_unsupported_levels_without_mutation() {
    use crate::{
        database::queries::application_settings, services::shared::settings::ApplicationSettings,
    };
    let (mut db, schema) = test_database().await;
    application_settings::update(
        &db,
        &ApplicationSettings {
            log_level: "debug".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        application_settings::read(&db).await.unwrap().log_level,
        "DEBUG"
    );
    let before = snapshot(&db, "application_settings").await;
    for level in ["OFF", "not-a-level"] {
        assert!(application_settings::update(
            &db,
            &ApplicationSettings {
                log_level: level.into()
            }
        )
        .await
        .is_err());
        assert_eq!(snapshot(&db, "application_settings").await, before);
    }
    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    assert_eq!(
        application_settings::read(&db).await.unwrap().log_level,
        "DEBUG"
    );
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_worker_changes_do_not_wait_for_import_finish_locks() {
    let (mut db, schema) = test_database().await;
    configure_scheduler(&mut db, &config()).await.unwrap();
    let url = std::env::var("TEST_POSTGRES_URL").unwrap();
    let (mut finishing, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    finishing
        .batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    let held = finishing.transaction().await.unwrap();
    held.query_one(
        "SELECT feed FROM price_feed_schedule WHERE feed = $1 FOR UPDATE",
        &[&Feed::Xetra.key()],
    )
    .await
    .unwrap();
    db.batch_execute("SET lock_timeout = '250ms'")
        .await
        .unwrap();
    scheduler::update(&mut db, true, 30, 3, &[Feed::Tradegate, Feed::Xetra])
        .await
        .unwrap();
    assert_eq!(scheduler::read(&db).await.unwrap().workers, 3);
    assert_config(&db, true, 30, &[Feed::Xetra, Feed::Tradegate]).await;
    held.rollback().await.unwrap();
    drop(finishing);
    cleanup(&db, &schema).await;
}
