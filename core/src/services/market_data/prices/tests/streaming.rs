use super::*;
use std::{io::Write, str::FromStr};

async fn seed_streamed_portfolio(db: &Client) -> HashSet<String> {
    db.batch_execute(
        "INSERT INTO instrument(user_id, id, name, price, last_price_update, fallback_price, fallback_at)
         VALUES('pricing-owner', 'US9999999999', 'Streamed equity', 1, '2020-01-01', 1, '2020-01-01');
         INSERT INTO trade(user_id, hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
             security_type, direction, currency, isin, date_added)
         VALUES('pricing-owner', 'streamed-holding', 'Test', now(), 1, 1, 1, 'Equity', 'Buy', 'EUR', 'US9999999999', now())",
    ).await.unwrap();
    let tracked = price::tracked_isins_all_users(db).await.unwrap();
    assert_eq!(tracked, HashSet::from(["US9999999999".into()]));
    tracked
}

fn streamed_row(index: i64, start: chrono::DateTime<Utc>) -> serde_json::Value {
    let event = start + ChronoDuration::microseconds(index);
    serde_json::json!({
        "messageId":"posttrade", "instrumentIdentificationCode":"US9999999999",
        "transactionIdentificationCode":format!("stream-{index}"),
        "venueOfPublication":"XCEF", "venueOfExecution":"XETA",
        "tradingDateAndTime":event, "publicationDateAndTime":event + ChronoDuration::milliseconds(1),
        "price":serde_json::Number::from_str("123.4567890123456789").unwrap(),
        "quantity":1, "priceCurrency":"EUR", "priceNotation":1,
        "mmtModificationInd":"-", "mmtTradingMode":"2"
    })
}

fn downloaded_rows(
    key: &str,
    rows: impl Iterator<Item = serde_json::Value>,
    malformed: bool,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> super::super::DownloadedFile {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for row in rows {
        writeln!(gzip, "{row}").unwrap();
    }
    if malformed {
        writeln!(gzip, "{{broken-json").unwrap();
    }
    let bytes = gzip.finish().unwrap();
    super::super::DownloadedFile {
        content_hash: blake3::hash(&bytes).to_hex().to_string(),
        bytes,
        etag: None,
        permit,
        file: types::SourceFile {
            key: key.into(),
            timestamp: Utc::now(),
            url: reqwest::Url::parse(&format!("https://mfs.deutsche-boerse.com/api/download/{key}"))
                .unwrap(),
        },
    }
}

fn streamed_file(
    malformed: bool,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> super::super::DownloadedFile {
    let start = Utc::now() - ChronoDuration::minutes(2);
    downloaded_rows("streamed-file", (0..4100).map(|index| streamed_row(index, start)), malformed, permit)
}

fn amendment_file(
    key: &str,
    indices: std::ops::Range<i64>,
    start: chrono::DateTime<Utc>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> super::super::DownloadedFile {
    let rows = indices.map(|index| {
        let mut row = streamed_row(index, start);
        if index != 0 {
            row["instrumentIdentificationCode"] = serde_json::json!("US8888888888");
        }
        if index >= 4096 {
            row["transactionIdentificationCode"] = serde_json::json!("stream-0");
            row["tradingDateAndTime"] = serde_json::json!(start);
            row["mmtModificationInd"] = serde_json::json!("A");
        }
        row
    });
    downloaded_rows(key, rows, false, permit)
}

async fn dry_run_snapshot(db: &Client) -> Vec<String> {
    let mut snapshot = Vec::new();
    for table in ["market_price_report", "mifid_latest_price", "instrument", "market_data_file", "background_job"] {
        snapshot.push(db.query_one(&format!(
            "SELECT coalesce(jsonb_agg(row_data ORDER BY row_data::text), '[]'::jsonb)::text
             FROM (SELECT to_jsonb(t) AS row_data FROM {table} t) snapshot"
        ), &[]).await.unwrap().get(0));
    }
    snapshot
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn malformed_tail_rolls_back_already_streamed_reports_and_checkpoint() {
    let (mut db, schema) = test_database().await;
    let tracked = seed_streamed_portfolio(&db).await;
    let (eligible, scope_hash) = (fixture_pairs(&tracked), price::portfolio_scope_hash(&tracked));
    // Sequence increments survive rollback, proving that the first admitted batch
    // actually reached SQL before the malformed tail aborted its transaction.
    db.batch_execute(
        "CREATE SEQUENCE streamed_insert_attempts;
         CREATE FUNCTION observe_streamed_insert() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM nextval('streamed_insert_attempts'); RETURN NEW; END $$;
         CREATE TRIGGER observe_streamed_insert BEFORE INSERT ON market_price_report
             FOR EACH ROW EXECUTE FUNCTION observe_streamed_insert()",
    )
    .await
    .unwrap();
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let permit = std::sync::Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let result = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::Persist(&job), streamed_file(true, permit), &tracked, &eligible, &scope_hash)
    .await;
    assert!(result.is_err());
    assert_eq!(
        db.query_one("SELECT last_value FROM streamed_insert_attempts", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        4096
    );
    let stored = db
        .query_one(
            "SELECT (SELECT count(*) FROM market_price_report WHERE isin='US9999999999'),
             (SELECT count(*) FROM market_data_file WHERE file_key='streamed-file'),
             files_processed, reports_processed FROM background_job WHERE id=$1",
            &[&job.id],
        )
        .await
        .unwrap();
    assert_eq!(stored.get::<_, i64>(0), 0);
    assert_eq!(stored.get::<_, i64>(1), 0);
    assert_eq!(stored.get::<_, i32>(2), 0);
    assert_eq!(stored.get::<_, i32>(3), 0);
    assert!(price::latest_prices(&db, &["US9999999999".into()]).await.unwrap().is_empty());
    assert_eq!(db.query_one("SELECT count(*) FROM mifid_latest_price", &[]).await.unwrap().get::<_, i64>(0), 0);
    let selected = db
        .query_one(
            "SELECT price, price_report_id FROM instrument WHERE user_id = $1 AND id='US9999999999'",
            &[&owner().as_str()],
        )
        .await
        .unwrap();
    assert_eq!(selected.get::<_, Decimal>(0), dec!(1));
    assert_eq!(selected.get::<_, Option<String>>(1), None);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn multiple_streamed_batches_select_latest_portfolio_price_without_losing_precision() {
    let (mut db, schema) = test_database().await;
    let tracked = seed_streamed_portfolio(&db).await;
    let (eligible, scope_hash) = (fixture_pairs(&tracked), price::portfolio_scope_hash(&tracked));
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let permit = std::sync::Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let admitted = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::Persist(&job), streamed_file(false, permit), &tracked, &eligible, &scope_hash)
    .await
    .unwrap();
    assert_eq!(admitted, 4100);
    let quote = price::quotes(&owner(), &db, &["US9999999999".into()])
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        quote.price,
        Some(Decimal::from_str_exact("123.4567890123456789").unwrap())
    );
    let latest = price::latest_prices(&db, &["US9999999999".into()]).await.unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].price, Decimal::from_str_exact("123.4567890123456789").unwrap());
    assert_eq!(latest[0].currency, "EUR");
    assert_eq!(latest[0].feed, Feed::Xetra.key());
    assert_eq!(latest[0].venue, "XETA");
    assert_eq!(db.query_one("SELECT count(*) FROM mifid_latest_price", &[]).await.unwrap().get::<_, i64>(0), 1);
    let selected = db
        .query_one(
            "SELECT r.transaction_id, (SELECT count(*) FROM market_price_report),
             (SELECT count(*) FROM instrument), (SELECT count(*) FROM trade),
             j.files_processed, j.reports_processed, f.scope_hash
         FROM instrument i JOIN market_price_report r ON r.id=i.price_report_id
         JOIN background_job j ON j.id=$1
         JOIN market_data_file f ON f.feed=j.feed AND f.file_key='streamed-file'
             AND f.parser_version=$2 AND f.scope_hash=$3
         WHERE i.id='US9999999999'",
            &[&job.id, &price::PARSER_VERSION, &scope_hash],
        )
        .await
        .unwrap();
    assert_eq!(selected.get::<_, String>(0), "stream-4099");
    assert_eq!(selected.get::<_, i64>(1), 4100);
    assert_eq!(selected.get::<_, i64>(2), 1);
    assert_eq!(selected.get::<_, i64>(3), 1);
    assert_eq!(selected.get::<_, i32>(4), 1);
    assert_eq!(selected.get::<_, i32>(5), 4100);
    assert_eq!(selected.get::<_, String>(6), scope_hash);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn dry_run_matches_persistence_for_off_scope_amendments_across_streamed_batches() {
    let (mut db, schema) = test_database().await;
    let tracked = seed_streamed_portfolio(&db).await;
    let (eligible, scope_hash) = (fixture_pairs(&tracked), price::portfolio_scope_hash(&tracked));
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let before = dry_run_snapshot(&db).await;
    let downloads = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let start = Utc::now() - ChronoDuration::minutes(2);
    let mut state = Some(price::DryRunAdmissions::default());
    // The first 4096-row batch admits only stream-0. The next batch has two
    // revisions of that identity, both moved off scope, and must count only one.
    let dry_run = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::DryRun(&mut state), amendment_file("amended-batches", 0..4098, start, downloads.clone().acquire_owned().await.unwrap()), &tracked, &eligible, &scope_hash).await.unwrap();
    assert_eq!(dry_run, 2);
    assert!(state.is_some());
    assert_eq!(dry_run_snapshot(&db).await, before);
    let persisted = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::Persist(&job), amendment_file("amended-batches", 0..4098, start, downloads.acquire_owned().await.unwrap()), &tracked, &eligible, &scope_hash).await.unwrap();
    assert_eq!(dry_run, persisted);
    let report = db.query_one("SELECT isin, revision FROM market_price_report", &[]).await.unwrap();
    assert_eq!(report.get::<_, String>(0), "US8888888888");
    assert_eq!(report.get::<_, i32>(1), 1);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn dry_run_carries_admissions_across_consecutive_streamed_files_without_writes() {
    let (mut db, schema) = test_database().await;
    let tracked = seed_streamed_portfolio(&db).await;
    let (eligible, scope_hash) = (fixture_pairs(&tracked), price::portfolio_scope_hash(&tracked));
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let before = dry_run_snapshot(&db).await;
    let downloads = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let start = Utc::now() - ChronoDuration::minutes(2);
    let mut state = Some(price::DryRunAdmissions::default());
    let mut dry_run_counts = Vec::new();
    for (key, indices) in [("original-file", 0..4096), ("amendment-file", 4096..4098)] {
        let admitted = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::DryRun(&mut state), amendment_file(key, indices, start, downloads.clone().acquire_owned().await.unwrap()), &tracked, &eligible, &scope_hash).await.unwrap();
        dry_run_counts.push(admitted);
        assert_eq!(dry_run_snapshot(&db).await, before);
    }
    assert_eq!(dry_run_counts, [1, 1]);
    let mut persisted_counts = Vec::new();
    for (key, indices) in [("original-file", 0..4096), ("amendment-file", 4096..4098)] {
        persisted_counts.push(super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::Persist(&job), amendment_file(key, indices, start, downloads.clone().acquire_owned().await.unwrap()), &tracked, &eligible, &scope_hash).await.unwrap());
    }
    assert_eq!(dry_run_counts, persisted_counts);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn malformed_dry_run_file_invalidates_partial_refresh_admissions_without_writes() {
    let (mut db, schema) = test_database().await;
    let tracked = seed_streamed_portfolio(&db).await;
    let (eligible, scope_hash) = (fixture_pairs(&tracked), price::portfolio_scope_hash(&tracked));
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let before = dry_run_snapshot(&db).await;
    let downloads = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let mut state = Some(price::DryRunAdmissions::default());
    let result = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::DryRun(&mut state), streamed_file(true, downloads.clone().acquire_owned().await.unwrap()), &tracked, &eligible, &scope_hash).await;
    assert!(result.unwrap_err().to_string().contains("Parsing streamed-file"));
    assert!(state.is_none());
    assert_eq!(dry_run_snapshot(&db).await, before);
    let retry = super::super::ingest_file(&mut db, Feed::Xetra, super::super::FileIngestion::DryRun(&mut state), streamed_file(false, downloads.acquire_owned().await.unwrap()), &tracked, &eligible, &scope_hash).await;
    assert!(retry.unwrap_err().to_string().contains("Dry-run refresh already aborted"));
    assert_eq!(dry_run_snapshot(&db).await, before);
    cleanup(&db, &schema).await;
}
