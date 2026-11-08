use super::*;

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_latest_and_revision_retention_use_exact_source_time_boundaries() {
    let (mut db, schema) = test_database().await;
    let tx = db.transaction().await.unwrap();
    tx.batch_execute(
        "INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price, quantity,
             as_of, executed_ns, published_ns, revision, revision_hash, received_at)
         SELECT id, 'EUR', 'DETR-posttrade', id, 'XETA', 123.45, 1, event,
             (EXTRACT(EPOCH FROM event) * 1000000000)::bigint,
             (EXTRACT(EPOCH FROM event) * 1000000000)::bigint, 0, id, now()
         FROM (VALUES
             ('boundary', now() - interval '168 hours'),
             ('expired', now() - interval '168 hours 1 microsecond')
         ) fixtures(id, event);
         INSERT INTO mifid_price_revision(id, published_ns, revision, revision_hash, published_at)
         SELECT id, (EXTRACT(EPOCH FROM published) * 1000000000)::bigint, 2, id, published
         FROM (VALUES
             ('guard-boundary', now() - interval '7 days'),
             ('guard-expired', now() - interval '7 days 1 microsecond')
         ) fixtures(id, published);
         INSERT INTO mifid_price_floor(isin, currency, feed, executed_ns, published_ns, report_id)
         SELECT id, 'EUR', 'DETR-posttrade',
             (EXTRACT(EPOCH FROM event) * 1000000000)::bigint, 1, id
         FROM (VALUES
             ('floor-boundary', now() - interval '168 hours'),
             ('floor-expired', now() - interval '168 hours 1 microsecond')
         ) fixtures(id, event)",
    ).await.unwrap();
    assert!(!jobs::prune_in(&tx).await.unwrap());
    let latest: Vec<String> = tx.query("SELECT isin FROM mifid_latest_price ORDER BY isin", &[])
        .await.unwrap().into_iter().map(|row| row.get(0)).collect();
    assert_eq!(latest, ["boundary"]);
    let revisions: Vec<String> = tx.query("SELECT id FROM mifid_price_revision ORDER BY id", &[])
        .await.unwrap().into_iter().map(|row| row.get(0)).collect();
    assert_eq!(revisions, ["guard-boundary"]);
    let floors: Vec<String> = tx.query("SELECT isin FROM mifid_price_floor ORDER BY isin", &[])
        .await.unwrap().into_iter().map(|row| row.get(0)).collect();
    assert_eq!(floors, ["floor-boundary"]);
    tx.commit().await.unwrap();
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_latest_reads_exclude_expired_future_and_pre_split_events_before_cleanup() {
    let (db, schema) = test_database().await;
    db.batch_execute(
        "INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price, quantity,
             as_of, executed_ns, published_ns, revision, revision_hash, received_at)
         SELECT id, 'EUR', 'DETR-posttrade', id, 'XETA', 123.45, 1, event,
             (EXTRACT(EPOCH FROM event) * 1000000000)::bigint,
             (EXTRACT(EPOCH FROM published) * 1000000000)::bigint, 0, id, now()
         FROM (VALUES
             ('valid', now() - interval '1 hour', now() - interval '59 minutes'),
             ('expired', now() - interval '169 hours', now() - interval '1 minute'),
             ('future-execution', now() + interval '1 hour', now() + interval '1 hour'),
             ('future-publication', now() - interval '1 minute', now() + interval '1 hour'),
             ('split', now() - interval '2 hours', now() - interval '1 hour'),
             ('future-split', now() - interval '1 hour', now() - interval '1 hour')
         ) fixtures(id, event, published);
         INSERT INTO stock_split(id, ex_date, from_factor, to_factor, isin, date_added) VALUES
             ('effective', now() - interval '90 minutes', 1, 2, 'split', now()),
             ('not-effective', now() + interval '1 day', 1, 2, 'future-split', now());
         INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price, quantity,
             as_of, executed_ns, published_ns, revision, revision_hash, received_at)
         SELECT 'split', 'USD', 'DETR-posttrade', 'post-split', 'XETA', 125, 1,
             now() - interval '1 hour',
             (EXTRACT(EPOCH FROM now() - interval '1 hour') * 1000000000)::bigint,
             (EXTRACT(EPOCH FROM now() - interval '1 hour') * 1000000000)::bigint,
             0, 'post-split', now()",
    ).await.unwrap();
    let requested: Vec<String> = ["valid", "expired", "future-execution", "future-publication", "split", "future-split"]
        .into_iter().map(str::to_owned).collect();
    let mut quotes = price::latest_prices(&db, &requested).await.unwrap();
    quotes.sort_by(|a, b| a.isin.cmp(&b.isin));
    assert_eq!(
        quotes.iter().map(|quote| (quote.isin.as_str(), quote.currency.as_str(), quote.price)).collect::<Vec<_>>(),
        vec![
            ("future-split", "EUR", dec!(123.45)),
            ("split", "USD", dec!(125)),
            ("valid", "EUR", dec!(123.45)),
        ],
    );
    // Read-time validity is independent of whether maintenance has touched storage.
    assert_eq!(db.query_one("SELECT count(*) FROM mifid_latest_price", &[]).await.unwrap().get::<_, i64>(0), 7);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_replay_does_not_renew_events_or_revision_publication_retention() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let event = Utc::now() - ChronoDuration::hours(167);
    let mut amended = super::quotes::event_report("amended", "unheld", event, dec!(42));
    amended.revision = 1;
    amended.published_ns += 1000;
    let mut expired = super::quotes::event_report(
        "expired", "expired-isin", Utc::now() - ChronoDuration::hours(169), dec!(99),
    );
    expired.revision = 2;
    expired.eligible = false;
    let expired_trade = super::quotes::event_report(
        "expired-trade", "expired-isin", Utc::now() - ChronoDuration::hours(169), dec!(98),
    );
    let rows = vec![amended.clone(), expired, expired_trade.clone()];
    write_file(&mut db, &job, "first", "a", None, rows.clone()).await.unwrap();
    write_file(&mut db, &job, "replay", "a", None, rows).await.unwrap();
    let quote = price::latest_prices(&db, &["unheld".into(), "expired-isin".into()]).await.unwrap();
    assert_eq!(quote.len(), 1);
    assert_eq!(quote[0].price, dec!(42));
    assert_eq!(quote[0].as_of.timestamp_micros(), event.timestamp_micros());
    let publication: chrono::DateTime<Utc> = db.query_one(
        "SELECT published_at FROM mifid_price_revision WHERE id='amended'", &[],
    ).await.unwrap().get(0);
    assert_eq!(publication.timestamp_micros(), amended.published_ns / 1000);
    let tx = db.transaction().await.unwrap();
    jobs::prune_in(&tx).await.unwrap();
    assert_eq!(tx.query_one("SELECT count(*) FROM mifid_price_revision WHERE id='expired'", &[])
        .await.unwrap().get::<_, i64>(0), 0);
    tx.commit().await.unwrap();
    write_file(&mut db, &job, "expired-replay-after-pruning", "b", None, vec![expired_trade]).await.unwrap();
    assert!(price::latest_prices(&db, &["expired-isin".into()]).await.unwrap().is_empty());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_source_tombstones_and_portfolio_references_obey_seven_day_retention() {
    let (mut db, schema) = test_database().await;
    let tx = db.transaction().await.unwrap();
    tx.batch_execute(
        "INSERT INTO market_price_report(id, feed, isin, transaction_id, publisher, venue,
             executed_at, executed_ns, published_ns, revision, revision_hash, price, currency,
             eligible, file_key, received_at)
         SELECT id, 'DETR-posttrade', 'DE0007164600', id, 'XCEF', 'XETA',
             now() - interval '9 days', 1, 1, revision, id, 50, 'EUR', revision <> 2, 'saved', received
         FROM (VALUES
             ('tombstone-boundary', 2, now() - interval '7 days'),
             ('tombstone-expired', 2, now() - interval '7 days 1 microsecond'),
             ('old-global-unused', 0, now() - interval '8 days'),
             ('old-event-new-revision', 1, now()),
             ('portfolio-protected', 0, now() - interval '30 days')
         ) reports(id, revision, received);
         INSERT INTO instrument(user_id, id, name, price, last_price_update, price_pinned, price_report_id)
         VALUES('pricing-owner', 'DE0007164600', 'Pinned portfolio', 50, '2020-01-01', TRUE, 'portfolio-protected')",
    ).await.unwrap();
    assert!(!jobs::prune_in(&tx).await.unwrap());
    let retained: Vec<String> = tx
        .query("SELECT id FROM market_price_report ORDER BY id", &[])
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(
        retained,
        [
            "old-event-new-revision",
            "portfolio-protected",
            "tombstone-boundary"
        ]
    );
    let protected = tx
        .query_one(
            "SELECT price, price_pinned, price_report_id FROM instrument WHERE user_id = $1 AND id='DE0007164600'",
            &[&owner().as_str()],
        )
        .await
        .unwrap();
    assert_eq!(protected.get::<_, Decimal>(0), dec!(50));
    assert!(protected.get::<_, bool>(1));
    assert_eq!(protected.get::<_, String>(2), "portfolio-protected");
    tx.commit().await.unwrap();
    let portfolio = price::quotes(&owner(), &db, &["DE0007164600".into()])
        .await
        .unwrap()
        .remove(0);
    assert!(portfolio.pinned);
    assert_eq!(portfolio.price, Some(dec!(50)));
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_checkpoint_cleanup_is_scope_safe_and_terminal_jobs_keep_fourteen_days() {
    let (mut db, schema) = test_database().await;
    jobs::ensure_feeds(&db, &[Feed::Xetra, Feed::Cboe])
        .await
        .unwrap();
    let tx = db.transaction().await.unwrap();
    tx.batch_execute(
        "INSERT INTO market_data_file(feed, file_key, parser_version, scope_hash, content_hash, processed_at)
         VALUES
             ('DETR-posttrade', 'same-file', 'mifid-portfolio-v1', 'expired-scope', 'old', now() - interval '7 days 1 microsecond'),
             ('DETR-posttrade', 'same-file', 'mifid-portfolio-v1', 'fresh-scope', 'fresh', now()),
             ('DETR-posttrade', 'same-file', 'other-parser', 'fresh-scope', 'other', now()),
             ('DETR-posttrade', 'same-file', 'mifid-portfolio-v1', 'boundary-scope', 'boundary', now() - interval '7 days');
         INSERT INTO background_job(feed, status, finished_at, last_error) VALUES
             ('DETR-posttrade', 'succeeded', now() - interval '14 days', 'boundary'),
             ('DETR-posttrade', 'failed', now() - interval '14 days 1 microsecond', 'expired'),
             ('CBOE-posttrade', 'queued', now() - interval '30 days', 'active')",
    ).await.unwrap();
    assert!(!jobs::prune_in(&tx).await.unwrap());
    let files: Vec<(String, String, String)> = tx.query(
        "SELECT parser_version, scope_hash, content_hash FROM market_data_file ORDER BY parser_version, scope_hash", &[],
    ).await.unwrap().into_iter().map(|row| (row.get(0), row.get(1), row.get(2))).collect();
    assert_eq!(
        files,
        vec![
            (
                "mifid-portfolio-v1".into(),
                "boundary-scope".into(),
                "boundary".into()
            ),
            (
                "mifid-portfolio-v1".into(),
                "fresh-scope".into(),
                "fresh".into()
            ),
            ("other-parser".into(), "fresh-scope".into(), "other".into()),
        ]
    );
    let jobs: Vec<String> = tx
        .query(
            "SELECT last_error FROM background_job ORDER BY last_error",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(jobs, ["active", "boundary"]);
    tx.commit().await.unwrap();
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_paused_startup_retention_drains_multiple_batches_independently() {
    let (mut db, schema) = test_database().await;
    configure_scheduler(
        &mut db,
        &PriceConfig {
            enabled: false,
            ..config()
        },
    )
    .await
    .unwrap();
    db.batch_execute(
        "INSERT INTO market_price_report(id, feed, isin, transaction_id, publisher, venue,
             executed_at, executed_ns, published_ns, revision, revision_hash, price, currency,
             eligible, file_key, received_at)
         SELECT 'old-' || n, 'DETR-posttrade', 'isin-' || n, 'trade-' || n, 'XCEF', 'XETA',
             now() - interval '8 days', 1, 1, 0, 'old-' || n, 50, 'EUR', TRUE, 'old-file', now() - interval '8 days'
         FROM generate_series(1, 10002) n;
         INSERT INTO instrument(user_id, id, name, price, last_price_update, price_pinned, price_report_id)
         VALUES('pricing-owner', 'isin-1', 'Protected holding', 50, '2020-01-01', TRUE, 'old-1');
         INSERT INTO background_job(feed, status, finished_at, last_error) VALUES
             ('DETR-posttrade', 'failed', now(), 'paused failure'),
             ('DGAT-posttrade', 'queued', NULL, 'paused queued')",
    ).await.unwrap();
    db.batch_execute(
        "INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price,
             as_of, executed_ns, published_ns, revision, revision_hash, received_at)
         SELECT 'expired-' || n, 'EUR', 'DETR-posttrade', 'expired-' || n, 'XETA', 50,
             now() - interval '169 hours', 1, 1, 0, 'expired-' || n, now()
         FROM generate_series(1, 10001) n;
         INSERT INTO mifid_price_revision(id, published_ns, revision, revision_hash, published_at)
         SELECT 'expired-' || n, 1, 2, 'expired-' || n, now() - interval '8 days'
         FROM generate_series(1, 15001) n;
         INSERT INTO mifid_price_floor(isin, currency, feed, executed_ns, published_ns, report_id)
         SELECT 'expired-' || n, 'EUR', 'DETR-posttrade', 1, 1, 'expired-' || n
         FROM generate_series(1, 10001) n",
    ).await.unwrap();
    let before_jobs: Vec<String> = db
        .query(
            "SELECT to_jsonb(j)::text FROM background_job j ORDER BY id",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    let before_schedule: Vec<String> = db
        .query(
            "SELECT to_jsonb(s)::text FROM price_feed_schedule s ORDER BY feed",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let connection = connect_schema(&schema).await;
    let maintenance = tokio::spawn(super::super::retention_loop_with_connection(
        receiver,
        Some(connection),
    ));
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let remaining = db
                .query_one(
                    "SELECT (SELECT count(*) FROM mifid_latest_price),
                     (SELECT count(*) FROM market_price_report),
                     (SELECT count(*) FROM mifid_price_revision),
                     (SELECT count(*) FROM mifid_price_floor)",
                    &[],
                )
                .await
                .unwrap();
            if remaining.get::<_, i64>(0) == 0
                && remaining.get::<_, i64>(1) == 1
                && remaining.get::<_, i64>(2) == 0
                && remaining.get::<_, i64>(3) == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Startup maintenance must drain more than one batch while scheduling is paused");
    stop.send(true).unwrap();
    maintenance.await.unwrap();
    assert!(!price_scheduler::read(&db).await.unwrap().enabled);
    let after_jobs: Vec<String> = db
        .query(
            "SELECT to_jsonb(j)::text FROM background_job j ORDER BY id",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    let after_schedule: Vec<String> = db
        .query(
            "SELECT to_jsonb(s)::text FROM price_feed_schedule s ORDER BY feed",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(after_jobs, before_jobs);
    assert_eq!(after_schedule, before_schedule);
    assert_eq!(
        db.query_one("SELECT id FROM market_price_report", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "old-1"
    );
    let portfolio = price::quotes(&owner(), &db, &["isin-1".into()])
        .await
        .unwrap()
        .remove(0);
    assert!(portfolio.pinned);
    assert_eq!(portfolio.price, Some(dec!(50)));
    cleanup(&db, &schema).await;
}
