use chrono::{Duration as ChronoDuration, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio_postgres::{Client, NoTls};

use super::*;
use crate::database::queries::{background_job as jobs, price, price_scheduler};

mod scheduler;

#[test]
fn retries_back_off_and_respect_provider_retry_after() {
    let mut job = PriceJob {
        id: 1,
        feed: Feed::Xetra.key().into(),
        attempts: 1,
        lease_token: 1,
    };
    let error = anyhow::anyhow!("temporary failure");
    assert!((60..75).contains(&retry_delay(&job, &error)));
    job.attempts = 2;
    assert!((300..315).contains(&retry_delay(&job, &error)));
    let error = anyhow::Error::new(http::RateLimited {
        retry_after_seconds: 1200,
    });
    assert_eq!(retry_delay(&job, &error), 1200);
}

async fn test_database() -> (Client, String) {
    test_database_at(refinery::Target::Latest).await
}

async fn test_database_at(target: refinery::Target) -> (Client, String) {
    static NEXT_SCHEMA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Never fall back to POSTGRES_URL or a developer's .env database.
    let url = std::env::var("TEST_POSTGRES_URL")
        .expect("Set TEST_POSTGRES_URL to an isolated PostgreSQL instance");
    let (mut db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    let schema = format!(
        "pricing_test_{}_{}_{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap(),
        NEXT_SCHEMA.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    db.batch_execute(&format!(
        "CREATE SCHEMA {schema}; SET search_path TO {schema}"
    ))
    .await
    .unwrap();
    refinery::embed_migrations!("migrations");
    migrations::runner()
        .set_target(target)
        .run_async(&mut db)
        .await
        .unwrap();
    (db, schema)
}

async fn cleanup(db: &Client, schema: &str) {
    db.batch_execute(&format!(
        "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
    ))
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_migrations_preserve_custom_assets_on_upgrade() {
    let (mut db, schema) = test_database_at(refinery::Target::Version(9)).await;
    db.batch_execute(
        "INSERT INTO asset (id, name, asset_class, currency, unit_label)
         VALUES ('saved-asset', 'Gold', 'PhysicalGold', 'EUR', 'oz');
         INSERT INTO instrument (id, name, price, last_price_update)
         VALUES ('saved-instrument', 'Existing holding', 42.5, '2026-10-16T12:00:00Z')",
    )
    .await
    .unwrap();
    let history_sql =
        "SELECT name, checksum, applied_on FROM refinery_schema_history WHERE version = 9";
    let history = db.query_one(history_sql, &[]).await.unwrap();
    let original_history = (
        history.get::<_, String>(0),
        history.get::<_, String>(1),
        history.get::<_, String>(2),
    );
    assert_eq!(original_history.0, "custom_assets");

    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    let history = db.query_one(history_sql, &[]).await.unwrap();
    assert_eq!(
        (
            history.get::<_, String>(0),
            history.get::<_, String>(1),
            history.get::<_, String>(2),
        ),
        original_history
    );
    let asset = db
        .query_one("SELECT name FROM asset WHERE id = 'saved-asset'", &[])
        .await
        .unwrap();
    assert_eq!(asset.get::<_, String>(0), "Gold");
    let price = db
        .query_one(
            "SELECT price, fallback_price, last_price_update = fallback_at
             FROM instrument WHERE id = 'saved-instrument'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(price.get::<_, Decimal>(0), dec!(42.5));
    assert_eq!(price.get::<_, Decimal>(1), dec!(42.5));
    assert!(price.get::<_, bool>(2));
    let rerun = migrations::runner().run_async(&mut db).await.unwrap();
    assert!(rerun.applied_migrations().is_empty());
    cleanup(&db, &schema).await;
}

fn config() -> PriceConfig {
    PriceConfig {
        enabled: true,
        interval_minutes: 30,
        workers: 2,
        feeds: vec![Feed::Xetra, Feed::Tradegate],
    }
}

async fn configure_scheduler(db: &mut Client, config: &PriceConfig) -> anyhow::Result<()> {
    price_scheduler::initialize(db).await?;
    price_scheduler::update(
        db,
        config.enabled,
        config.interval_minutes,
        config.workers,
        &config.feeds,
    )
    .await
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_queue_recovers_leases_and_coalesces_requests() {
    let (mut db, schema) = test_database().await;
    configure_scheduler(&mut db, &config()).await.unwrap();
    price_scheduler::schedule_due(&mut db).await.unwrap();
    price_scheduler::schedule_due(&mut db).await.unwrap();
    db.execute(
        "UPDATE price_feed_schedule SET next_refresh_at = now() - interval '6 hours'",
        &[],
    )
    .await
    .unwrap();
    price_scheduler::schedule_due(&mut db).await.unwrap();
    let scheduled_seconds: f64 = db.query_one("SELECT EXTRACT(EPOCH FROM next_refresh_at - now())::float8 FROM price_feed_schedule WHERE enabled LIMIT 1", &[]).await.unwrap().get(0);
    assert!((1790.0..=1800.0).contains(&scheduled_seconds));
    let queued: i64 = db
        .query_one("SELECT count(*) FROM background_job", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(queued, 2);
    let first = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    assert!(jobs::claim(&db, &[Feed::Tradegate])
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap(),
        [first.id]
    );
    db.execute(
        "UPDATE background_job SET lease_until = now() - interval '1 second' WHERE id = $1",
        &[&first.id],
    )
    .await
    .unwrap();
    let recovered = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(recovered.id, first.id);
    assert!(recovered.lease_token > first.lease_token);
    assert_eq!(recovered.attempts, 2);
    assert!(jobs::renew(&db, &first).await.is_err());
    assert!(jobs::finish(&mut db, &first).await.is_err());
    assert!(price::commit_file(
        &mut db,
        &first,
        "expired-file",
        "universe",
        "hash",
        None,
        vec![]
    )
    .await
    .is_err());
    let checkpoint_count: i64 = db
        .query_one("SELECT count(*) FROM market_data_file", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(checkpoint_count, 0);
    jobs::finish(&mut db, &recovered).await.unwrap();
    // The manual request during the running job coalesced into one follow-up.
    let follow_up = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_ne!(follow_up.id, recovered.id);
    jobs::fail(&mut db, &follow_up, "temporary error", 60)
        .await
        .unwrap();
    // A manual refresh must not bypass a provider's Retry-After/backoff window.
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    db.execute(
        "UPDATE background_job SET available_at = now() - interval '1 second' WHERE id = $1",
        &[&follow_up.id],
    )
    .await
    .unwrap();
    let retry = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(retry.attempts, 2);
    db.execute("UPDATE background_job SET attempts = 4, lease_until = now() - interval '1 second' WHERE id = $1", &[&retry.id]).await.unwrap();
    assert!(jobs::claim(&db, &[Feed::Xetra]).await.unwrap().is_none());
    let state: String = db
        .query_one(
            "SELECT status FROM background_job WHERE id = $1",
            &[&retry.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(state, "failed");
    assert!(jobs::enqueue(&mut db, &[Feed::Xetra]).await.is_ok());
    let status = price::feed_status(&db).await.unwrap();
    assert!(status.iter().any(|s| s.last_success_at.is_some()));
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_manual_refresh_replaces_only_expired_exhausted_jobs() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Frankfurt]).await.unwrap();
    let old = jobs::claim(&db, &[Feed::Frankfurt]).await.unwrap().unwrap();
    db.execute(
        "UPDATE background_job SET attempts = 4 WHERE id = $1",
        &[&old.id],
    )
    .await
    .unwrap();
    // A valid final-attempt lease still belongs to its current worker.
    assert_eq!(
        jobs::enqueue(&mut db, &[Feed::Frankfurt]).await.unwrap(),
        [old.id]
    );
    db.execute(
        "UPDATE background_job SET lease_until = clock_timestamp() - interval '1 second'
         WHERE id = $1",
        &[&old.id],
    )
    .await
    .unwrap();
    let replacement = jobs::enqueue(&mut db, &[Feed::Frankfurt]).await.unwrap();
    assert_ne!(replacement, [old.id]);
    let terminal = db
        .query_one(
            "SELECT status, finished_at IS NOT NULL AS finished FROM background_job WHERE id = $1",
            &[&old.id],
        )
        .await
        .unwrap();
    assert_eq!(terminal.get::<_, String>("status"), "failed");
    assert!(terminal.get::<_, bool>("finished"));
    let fresh = jobs::claim(&db, &[Feed::Frankfurt]).await.unwrap().unwrap();
    assert_eq!(replacement, [fresh.id]);
    assert_eq!(fresh.attempts, 1);
    assert!(jobs::renew(&db, &old).await.is_err());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_interrupted_job_preserves_progress_and_fences_old_worker() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Frankfurt]).await.unwrap();
    let mut first = jobs::claim(&db, &[Feed::Frankfurt]).await.unwrap().unwrap();
    first.attempts = 4;
    db.execute(
        "UPDATE background_job SET attempts = $2 WHERE id = $1",
        &[&first.id, &first.attempts],
    )
    .await
    .unwrap();
    price::commit_file(
        &mut db,
        &first,
        "saved-batch",
        "universe",
        "hash",
        None,
        vec![],
    )
    .await
    .unwrap();
    jobs::release(&db, &first).await.unwrap();
    assert!(jobs::renew(&db, &first).await.is_err());
    let resumed = jobs::claim(&db, &[Feed::Frankfurt]).await.unwrap().unwrap();
    assert_eq!(resumed.id, first.id);
    assert_eq!(resumed.attempts, first.attempts);
    assert!(resumed.lease_token > first.lease_token);
    assert!(jobs::release(&db, &first).await.is_err());
    assert!(jobs::finish(&mut db, &first).await.is_err());
    let checkpoints = price::checkpoints(&db, Feed::Frankfurt.key(), "universe")
        .await
        .unwrap();
    assert_eq!(checkpoints["saved-batch"].content_hash, "hash");
    let row = db
        .query_one(
            "SELECT files_processed, status FROM background_job WHERE id = $1",
            &[&resumed.id],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i32>("files_processed"), 1);
    assert_eq!(row.get::<_, String>("status"), "running");
    // An expired worker may not requeue itself to escape the crash retry limit.
    db.execute(
        "UPDATE background_job SET lease_until = clock_timestamp() - interval '1 second'
         WHERE id = $1",
        &[&resumed.id],
    )
    .await
    .unwrap();
    assert!(jobs::release(&db, &resumed).await.is_err());
    cleanup(&db, &schema).await;
}

fn report(id: &str, minutes_ago: i64, value: Decimal) -> types::PriceReport {
    let executed_at = Utc::now() - ChronoDuration::minutes(minutes_ago);
    types::PriceReport {
        id: id.into(),
        isin: "DE0007164600".into(),
        transaction_id: id.into(),
        publisher: "XCEF".into(),
        venue: "XETA".into(),
        executed_at,
        executed_ns: executed_at.timestamp_nanos_opt().unwrap(),
        published_ns: executed_at.timestamp_nanos_opt().unwrap(),
        revision: 0,
        revision_hash: id.into(),
        price: value,
        currency: "EUR".into(),
        eligible: true,
    }
}

async fn selected(db: &Client) -> (Decimal, String, Option<String>) {
    let row = db
        .query_one(
            "SELECT price, price_source, price_report_id FROM instrument WHERE id = 'DE0007164600'",
            &[],
        )
        .await
        .unwrap();
    (row.get(0), row.get(1), row.get(2))
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_policy_replay_reselects_identical_reports_without_reviving_cancellations()
{
    let (mut db, schema) = test_database().await;
    db.execute(
        "INSERT INTO instrument(id, name, price, last_price_update, fallback_price, fallback_at)
         VALUES('DE0007164600','Example equity',50,'2020-01-01',50,'2020-01-01')",
        &[],
    )
    .await
    .unwrap();
    let tracked = std::collections::HashSet::from(["DE0007164600".into()]);
    let old_universe = blake3::hash(b"DE0007164600\n").to_hex().to_string();
    let universe = price::universe_hash(&tracked);
    jobs::enqueue(&mut db, &[Feed::Frankfurt]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Frankfurt]).await.unwrap().unwrap();
    let mut indication = report("zero-quantity-indication", 5, dec!(48));
    indication.venue = "FRAB".into();
    let mut previously_rejected = indication.clone();
    previously_rejected.eligible = false;
    price::commit_file(
        &mut db,
        &job,
        "minute",
        &old_universe,
        "same-content",
        None,
        vec![previously_rejected],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(50));
    assert!(price::checkpoints(&db, &job.feed, &universe)
        .await
        .unwrap()
        .is_empty());
    price::commit_file(
        &mut db,
        &job,
        "minute",
        &universe,
        "same-content",
        None,
        vec![indication.clone()],
    )
    .await
    .unwrap();
    assert_eq!(
        selected(&db).await,
        (
            dec!(48),
            "deutsche_boerse".into(),
            Some(indication.id.clone())
        )
    );
    let mut cancellation = indication.clone();
    cancellation.revision = 2;
    cancellation.published_ns += 1;
    cancellation.eligible = false;
    price::commit_file(
        &mut db,
        &job,
        "cancel",
        &universe,
        "cancel-content",
        None,
        vec![cancellation],
    )
    .await
    .unwrap();
    price::commit_file(
        &mut db,
        &job,
        "minute",
        &universe,
        "same-content",
        None,
        vec![indication],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(50));
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_feed_fallback_respects_freshness_and_precedence() {
    let (mut db, schema) = test_database().await;
    let feeds = [
        Feed::Xetra,
        Feed::Tradegate,
        Feed::Frankfurt,
        Feed::Cboe,
        Feed::Euronext,
    ];
    configure_scheduler(
        &mut db,
        &PriceConfig {
            feeds: feeds.to_vec(),
            ..config()
        },
    )
    .await
    .unwrap();
    price_scheduler::schedule_due(&mut db).await.unwrap();
    let mut feed_jobs = Vec::new();
    for feed in feeds {
        feed_jobs.push(jobs::claim(&db, &[feed]).await.unwrap().unwrap());
    }
    price::ensure_instruments(
        &db,
        &std::collections::HashSet::from(["DE0007164600".into()]),
    )
    .await
    .unwrap();
    let mut euronext = report("euronext", 1, dec!(105));
    euronext.venue = "XPAR".into();
    let mut cboe = report("cboe", 2, dec!(104));
    cboe.venue = "CEUX".into();
    let mut frankfurt = report("frankfurt", 5, dec!(103));
    frankfurt.venue = "FRAB".into();
    let mut tradegate = report("tradegate", 10, dec!(102));
    tradegate.venue = "XGAT".into();
    let stale_xetra = report("stale-xetra", 120, dec!(90));
    let xetra = report("xetra", 20, dec!(101));
    let mut cancel_xetra = xetra.clone();
    cancel_xetra.revision = 2;
    cancel_xetra.eligible = false;
    cancel_xetra.published_ns += 1000;
    let mut cancel_tradegate = tradegate.clone();
    cancel_tradegate.revision = 2;
    cancel_tradegate.eligible = false;
    cancel_tradegate.published_ns += 1000;
    let mut cancel_frankfurt = frankfurt.clone();
    cancel_frankfurt.revision = 2;
    cancel_frankfurt.eligible = false;
    cancel_frankfurt.published_ns += 1000;
    let mut cancel_cboe = cboe.clone();
    cancel_cboe.revision = 2;
    cancel_cboe.eligible = false;
    cancel_cboe.published_ns += 1000;
    let cases = [
        (
            Feed::Euronext,
            euronext.clone(),
            Feed::Euronext,
            euronext.clone(),
        ),
        (Feed::Cboe, cboe.clone(), Feed::Cboe, cboe.clone()),
        (
            Feed::Frankfurt,
            frankfurt.clone(),
            Feed::Frankfurt,
            frankfurt.clone(),
        ),
        (
            Feed::Tradegate,
            tradegate.clone(),
            Feed::Tradegate,
            tradegate.clone(),
        ),
        (Feed::Xetra, stale_xetra, Feed::Tradegate, tradegate.clone()),
        (Feed::Xetra, xetra.clone(), Feed::Xetra, xetra),
        (Feed::Xetra, cancel_xetra, Feed::Tradegate, tradegate),
        (
            Feed::Tradegate,
            cancel_tradegate,
            Feed::Frankfurt,
            frankfurt,
        ),
        (Feed::Frankfurt, cancel_frankfurt, Feed::Cboe, cboe),
        (Feed::Cboe, cancel_cboe, Feed::Euronext, euronext),
    ];
    for (index, (feed, change, expected_feed, expected)) in cases.into_iter().enumerate() {
        let job = feed_jobs.iter().find(|job| job.feed == feed.key()).unwrap();
        price::commit_file(
            &mut db,
            job,
            &format!("minute-{index}"),
            "universe",
            &format!("hash-{index}"),
            None,
            vec![change],
        )
        .await
        .unwrap();
        assert_eq!(
            selected(&db).await,
            (
                expected.price,
                expected_feed.source().into(),
                Some(expected.id),
            )
        );
        let quotes = price::quotes(&db, &["DE0007164600".into()]).await.unwrap();
        assert_eq!(quotes[0].venue.as_deref(), Some(expected.venue.as_str()));
    }
    for job in &feed_jobs {
        jobs::finish(&mut db, job).await.unwrap();
    }
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_revisions_pins_and_checkpoints_are_atomic() {
    let (mut db, schema) = test_database().await;
    db.execute(
        "INSERT INTO instrument(id, name, price, last_price_update, fallback_price, fallback_at)
        VALUES('DE0007164600','Example equity',50,'2020-01-01',50,'2020-01-01')",
        &[],
    )
    .await
    .unwrap();
    let ids = jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    assert_eq!(ids, [job.id]);
    let first = report("first", 20, dec!(100));
    let second = report("second", 10, dec!(101));
    price::commit_file(
        &mut db,
        &job,
        "minute-1",
        "universe",
        "hash1",
        None,
        vec![first.clone(), second.clone()],
    )
    .await
    .unwrap();
    assert_eq!(
        selected(&db).await,
        (dec!(101), "deutsche_boerse".into(), Some("second".into()))
    );
    // Replaying old files and duplicate rows does not roll the current price back.
    price::commit_file(
        &mut db,
        &job,
        "daily",
        "universe",
        "hash2",
        None,
        vec![first.clone(), first.clone()],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(101));
    let mut cancellation = second.clone();
    cancellation.revision = 2;
    cancellation.eligible = false;
    cancellation.published_ns += 1000;
    price::commit_file(
        &mut db,
        &job,
        "cancellation",
        "universe",
        "hash3",
        None,
        vec![cancellation],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(100));
    let late_original = report("cancelled-before-original", 5, dec!(999));
    let mut tombstone = late_original.clone();
    tombstone.revision = 2;
    tombstone.eligible = false;
    tombstone.published_ns += 1000;
    price::commit_file(
        &mut db,
        &job,
        "tombstone",
        "universe",
        "hash-tombstone",
        None,
        vec![tombstone],
    )
    .await
    .unwrap();
    price::commit_file(
        &mut db,
        &job,
        "original-after-tombstone",
        "universe",
        "hash-original",
        None,
        vec![late_original],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(100));
    price::commit_file(
        &mut db,
        &job,
        "late-original",
        "universe",
        "hash4",
        None,
        vec![second],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(100));
    let mut amendment = first.clone();
    amendment.revision = 1;
    amendment.price = dec!(99);
    amendment.published_ns += 2000;
    price::commit_file(
        &mut db,
        &job,
        "amendment",
        "universe",
        "hash5",
        None,
        vec![amendment],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(99));
    // Identical price on a newer trade still advances provenance and the timestamp.
    let newer = report("newer", 5, dec!(99));
    price::commit_file(
        &mut db,
        &job,
        "newer",
        "universe",
        "hash6",
        None,
        vec![newer],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.2.as_deref(), Some("newer"));
    price::set_manual(&mut db, "DE0007164600", dec!(123), Utc::now())
        .await
        .unwrap();
    price::commit_file(
        &mut db,
        &job,
        "pinned",
        "universe",
        "hash7",
        None,
        vec![report("latest", 1, dec!(105))],
    )
    .await
    .unwrap();
    assert_eq!(selected(&db).await.0, dec!(123));
    price::unpin(&mut db, "DE0007164600").await.unwrap();
    assert_eq!(selected(&db).await.0, dec!(105));
    let quotes = price::quotes(
        &db,
        &[
            "DE0007164600".into(),
            "unknown".into(),
            "DE0007164600".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(quotes[0].venue.as_deref(), Some("XETA"));
    assert!(quotes[1].price.is_none());
    assert_eq!(quotes[0].price, quotes[2].price);
    assert_eq!(
        price::checkpoints(&db, &job.feed, "universe")
            .await
            .unwrap()
            .len(),
        9
    );
    assert!(price::checkpoints(&db, &job.feed, "new-universe")
        .await
        .unwrap()
        .is_empty());
    // A split makes old unadjusted quotes and fallbacks unusable; don't multiply a
    // post-split inventory by a pre-split price or silently invent an adjusted quote.
    db.execute(
        "INSERT INTO stock_split(id, ex_date, from_factor, to_factor, isin, date_added)
        VALUES('split',now(),1,2,'DE0007164600',now())",
        &[],
    )
    .await
    .unwrap();
    // Read paths invalidate a known split immediately, before the next worker runs.
    assert!(
        price::quotes(&db, &["DE0007164600".into()]).await.unwrap()[0]
            .price
            .is_none()
    );
    price::reselect_all(
        &mut db,
        &job,
        &std::collections::HashSet::from(["DE0007164600".into()]),
    )
    .await
    .unwrap();
    assert!(
        price::quotes(&db, &["DE0007164600".into()]).await.unwrap()[0]
            .price
            .is_none()
    );
    jobs::finish(&mut db, &job).await.unwrap();
    assert!(price::commit_file(
        &mut db,
        &job,
        "after-finish",
        "universe",
        "hash",
        None,
        vec![]
    )
    .await
    .is_err());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_claim_skips_locked_work() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra, Feed::Tradegate])
        .await
        .unwrap();
    let (mut other, connection) =
        tokio_postgres::connect(&std::env::var("TEST_POSTGRES_URL").unwrap(), NoTls)
            .await
            .unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    other
        .batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    let tx = db.transaction().await.unwrap();
    tx.query_one(
        "SELECT id FROM background_job WHERE feed = 'DETR-posttrade' FOR UPDATE",
        &[],
    )
    .await
    .unwrap();
    let claimed = tokio::time::timeout(
        Duration::from_secs(2),
        jobs::claim(&other, &[Feed::Xetra, Feed::Tradegate]),
    )
    .await
    .expect("Claim must skip locked rows, not block")
    .unwrap()
    .unwrap();
    assert_eq!(claimed.feed, Feed::Tradegate.key());
    tx.rollback().await.unwrap();
    assert_eq!(
        jobs::claim(&other, &[Feed::Xetra, Feed::Tradegate])
            .await
            .unwrap()
            .unwrap()
            .feed,
        Feed::Xetra.key()
    );
    assert!(jobs::claim(&other, &[Feed::Xetra, Feed::Tradegate])
        .await
        .unwrap()
        .is_none());
    let forward = [Feed::Xetra, Feed::Tradegate];
    let reverse = [Feed::Tradegate, Feed::Xetra];
    let (first, second) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            jobs::enqueue(&mut db, &forward),
            jobs::enqueue(&mut other, &reverse)
        )
    })
    .await
    .expect("Concurrent reverse-order requests must not deadlock");
    assert_eq!(first.unwrap(), second.unwrap());
    db.execute(
        "UPDATE background_job SET attempts = 4, lease_until = now() - interval '1 second'
        WHERE feed = 'DETR-posttrade' AND status = 'running'",
        &[],
    )
    .await
    .unwrap();
    let tx = db.transaction().await.unwrap();
    tx.query_one("SELECT id FROM background_job WHERE feed = 'DETR-posttrade' AND status = 'running' FOR UPDATE", &[]).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), jobs::claim(&other, &forward))
            .await
            .expect("Exhausted-job recovery must also skip locked rows")
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    assert!(jobs::claim(&other, &forward).await.unwrap().is_none());
    drop(other);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_imports_preserve_market_prices_and_manual_pins() {
    use crate::database::{
        models::instrument::Instrument, queries::instrument::record_execution_price,
    };

    for feed in [Feed::Xetra, Feed::Cboe, Feed::Euronext] {
        let (mut db, schema) = test_database().await;
        let mut execution = Instrument {
            id: "DE0007164600".into(),
            name: "Imported equity".into(),
            price: dec!(50),
            last_price_update: Utc::now() - ChronoDuration::days(1),
        };
        record_execution_price(&db, &execution).await.unwrap();
        jobs::enqueue(&mut db, &[feed]).await.unwrap();
        let job = jobs::claim(&db, &[feed]).await.unwrap().unwrap();
        price::commit_file(
            &mut db,
            &job,
            "market",
            "universe",
            "hash",
            None,
            vec![report("market", 5, dec!(100))],
        )
        .await
        .unwrap();
        let market_time = price::quotes(&db, std::slice::from_ref(&execution.id))
            .await
            .unwrap()[0]
            .as_of;
        execution.price = dec!(111);
        execution.last_price_update = Utc::now();
        record_execution_price(&db, &execution).await.unwrap();
        assert_eq!(
            selected(&db).await,
            (dec!(100), feed.source().into(), Some("market".into()))
        );
        assert_eq!(
            price::quotes(&db, std::slice::from_ref(&execution.id))
                .await
                .unwrap()[0]
                .as_of,
            market_time
        );
        let fallback: Decimal = db
            .query_one(
                "SELECT fallback_price FROM instrument WHERE id = $1",
                &[&execution.id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(fallback, dec!(111));
        price::set_manual(&mut db, &execution.id, dec!(123), Utc::now())
            .await
            .unwrap();
        execution.price = dec!(112);
        execution.last_price_update = Utc::now();
        record_execution_price(&db, &execution).await.unwrap();
        assert_eq!(selected(&db).await.0, dec!(123));
        // An older reimport cannot roll either the selected price or fallback backwards.
        execution.price = dec!(20);
        execution.last_price_update = Utc::now() - ChronoDuration::days(10);
        record_execution_price(&db, &execution).await.unwrap();
        assert_eq!(selected(&db).await.0, dec!(123));
        let fallback: Decimal = db
            .query_one(
                "SELECT fallback_price FROM instrument WHERE id = $1",
                &[&execution.id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(fallback, dec!(112));
        cleanup(&db, &schema).await;
    }
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_tracks_only_known_equities_and_replays_new_instruments() {
    let (db, schema) = test_database().await;
    for (hash, isin, security_type) in [
        ("equity", "equity-isin", "Equity"),
        ("bond", "bond-isin", "Bond"),
        ("derivative", "derivative-isin", "Derivative"),
        ("mixed-equity", "mixed-isin", "Equity"),
        ("mixed-bond", "mixed-isin", "Bond"),
        ("old-equity", "old-isin", "Equity"),
    ] {
        db.execute("INSERT INTO trade(hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
            security_type, direction, currency, isin, date_added)
            VALUES($1, 'Test', now(), 1, 1, 1, $3, 'Buy', 'EUR', $2, now())", &[&hash, &isin, &security_type]).await.unwrap();
    }
    db.execute("INSERT INTO listing_change(id, ex_date, from_factor, to_factor, from_identifier, to_identifier)
        VALUES('rename', '2020-01-01', 1, 1, 'old-isin', 'new-isin')", &[]).await.unwrap();
    let tracked = price::tracked_isins(&db).await.unwrap();
    assert_eq!(
        tracked,
        std::collections::HashSet::from(["equity-isin".into(), "new-isin".into()])
    );
    price::ensure_instruments(&db, &tracked).await.unwrap();
    assert!(price::quotes(&db, &["new-isin".into()]).await.unwrap()[0]
        .price
        .is_none());
    let old_fingerprint = price::universe_hash(&tracked);
    db.execute("INSERT INTO trade(hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
        security_type, direction, currency, isin, date_added)
        VALUES('new-equity', 'Test', now(), 1, 1, 1, 'Equity', 'Buy', 'EUR', 'another-equity', now())", &[]).await.unwrap();
    assert_ne!(
        old_fingerprint,
        price::universe_hash(&price::tracked_isins(&db).await.unwrap())
    );
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_instrument_catalog_and_rename_preserve_price_safety() {
    use crate::database::{
        models::instrument::Instrument,
        queries::instrument::{
            get_all_instruments, get_instrument_by_id, record_execution_price, rename_instrument,
        },
    };

    let (mut db, schema) = test_database().await;
    let instrument = Instrument {
        id: "DE0007164600".into(),
        name: "Imported equity".into(),
        price: dec!(50),
        last_price_update: Utc::now() - ChronoDuration::days(1),
    };
    record_execution_price(&db, &instrument).await.unwrap();
    db.batch_execute(
        "INSERT INTO instrument(id, name, price, last_price_update, price_source)
         VALUES ('unavailable', 'Unavailable unheld instrument', 25, '2020-01-01', 'unavailable'),
                ('split', 'Split unheld instrument', 100, '2020-01-01', 'execution_fallback');
         INSERT INTO stock_split(id, ex_date, from_factor, to_factor, isin, date_added)
         VALUES ('catalog-split', now(), 1, 2, 'split', now());",
    )
    .await
    .unwrap();
    let exact_price = dec!(123456789.12345678901234567890);
    price::set_manual(&mut db, &instrument.id, exact_price, Utc::now())
        .await
        .unwrap();
    let snapshot = "SELECT (to_jsonb(i) - 'name')::text FROM instrument i WHERE id = $1";
    let before: String = db
        .query_one(snapshot, &[&instrument.id])
        .await
        .unwrap()
        .get(0);
    let renamed = rename_instrument(&db, &instrument.id, "Renamed equity")
        .await
        .unwrap()
        .unwrap();
    let after: String = db
        .query_one(snapshot, &[&instrument.id])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after, "Rename must only change the name column");
    assert_eq!(renamed.name, "Renamed equity");
    assert_eq!(renamed.price, exact_price);
    assert_eq!(renamed.quote.price, Some(exact_price));
    assert_eq!(renamed.quote.source, "manual");
    assert!(renamed.quote.pinned);

    // No trades or positions exist: catalog quotes must include all known instruments.
    let catalog = get_all_instruments(&db).await.unwrap();
    assert_eq!(catalog.len(), 3);
    for isin in ["unavailable", "split"] {
        let details = catalog.iter().find(|entry| entry.id == isin).unwrap();
        assert!(
            details.price > Decimal::ZERO,
            "Legacy audit price is preserved"
        );
        assert_eq!(details.quote.source, "unavailable");
        assert_eq!(details.quote.price, None);
        assert_eq!(details.quote.as_of, None);
        let single = get_instrument_by_id(&db, isin).await.unwrap().unwrap();
        assert_eq!(single.quote.price, None);
        assert_eq!(single.quote.source, "unavailable");
    }
    assert!(get_instrument_by_id(&db, "missing")
        .await
        .unwrap()
        .is_none());
    assert!(rename_instrument(&db, "missing", "Not created")
        .await
        .unwrap()
        .is_none());

    price::unpin(&mut db, &instrument.id).await.unwrap();
    let unpinned = get_instrument_by_id(&db, &instrument.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unpinned.name, "Renamed equity");
    assert_eq!(unpinned.quote.price, Some(dec!(50)));
    assert_eq!(unpinned.quote.source, "execution_fallback");
    assert!(!unpinned.quote.pinned);
    cleanup(&db, &schema).await;
}
