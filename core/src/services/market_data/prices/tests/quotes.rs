use super::*;

pub(super) fn event_report(id: &str, isin: &str, event: chrono::DateTime<Utc>, value: Decimal) -> types::PriceReport {
    let mut row = report(id, 1, value);
    row.isin = isin.into();
    row.executed_at = event;
    row.executed_ns = event.timestamp_nanos_opt().unwrap();
    row.published_ns = row.executed_ns;
    row
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_latest_prices_cover_all_feeds_without_creating_portfolio_data() {
    let (mut db, schema) = test_database().await;
    let event = Utc::now() - ChronoDuration::minutes(5);
    let feeds = [Feed::Xetra, Feed::Tradegate, Feed::Frankfurt, Feed::Cboe, Feed::Euronext];
    jobs::enqueue(&mut db, &feeds).await.unwrap();
    for (index, feed) in feeds.into_iter().enumerate() {
        let job = jobs::claim(&db, &[feed]).await.unwrap().unwrap();
        let mut row = event_report(&format!("feed-{index}"), "unheld", event, Decimal::from(100 + index));
        row.venue = format!("venue-{index}");
        if index == 2 {
            row.quantity = Some(Decimal::ZERO);
        }
        let tracked = price::tracked_isins(&db).await.unwrap();
        assert!(tracked.is_empty());
        let scope_hash = price::portfolio_scope_hash(&tracked);
        let mut writer = price::begin_file(&mut db, &job, "bulk", &tracked, &scope_hash).await.unwrap();
        writer.push(std::slice::from_ref(&row)).await.unwrap();
        writer.finish("hash", None).await.unwrap();
        let quotes = price::latest_prices(&db, &["unheld".into(), "missing".into(), "unheld".into()]).await.unwrap();
        let quote = quotes.iter().find(|quote| quote.feed == feed.key()).unwrap();
        assert_eq!(quote.isin, "unheld");
        assert_eq!(quote.currency, "EUR");
        assert_eq!(quote.price, row.price);
        assert_eq!(quote.venue, row.venue);
        assert_eq!(quote.quantity, row.quantity);
        assert_eq!(quote.as_of.timestamp_micros(), event.timestamp_micros());
        assert!(quote.received_at >= event);
        assert_eq!(quotes.len(), index + 1);
    }
    let counts = db.query_one(
        "SELECT (SELECT count(*) FROM instrument), (SELECT count(*) FROM trade),
         (SELECT count(*) FROM market_price_report)", &[],
    ).await.unwrap();
    for index in 0..3 {
        assert_eq!(counts.get::<_, i64>(index), 0);
    }
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_latest_winner_uses_execution_time_across_batches_files_and_native_currencies() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let tracked = HashSet::new();
    let scope_hash = price::portfolio_scope_hash(&tracked);
    let event = Utc::now() - ChronoDuration::minutes(10);
    let older = event_report("older", "unheld", event, dec!(99));
    let newer = event_report("newer", "unheld", event + ChronoDuration::minutes(1), dec!(123.4567890123456789));
    let mut usd = newer.clone();
    usd.id = "usd".into();
    usd.currency = "USD".into();
    usd.price = dec!(130.25);
    let mut writer = price::begin_file(&mut db, &job, "batches", &tracked, &scope_hash).await.unwrap();
    writer.push(&[newer.clone(), usd.clone()]).await.unwrap();
    writer.push(std::slice::from_ref(&older)).await.unwrap();
    writer.finish("hash", None).await.unwrap();
    // A late publication of an older execution must not displace the newer trade.
    let mut amended_old = older.clone();
    amended_old.revision = 1;
    amended_old.published_ns = (event + ChronoDuration::minutes(5)).timestamp_nanos_opt().unwrap();
    amended_old.price = dec!(500);
    write_file(&mut db, &job, "late-amendment", "hash2", None, vec![amended_old]).await.unwrap();
    write_file(&mut db, &job, "old-replay", "hash3", None, vec![older]).await.unwrap();
    let quotes = price::latest_prices(&db, &["unheld".into()]).await.unwrap();
    assert_eq!(quotes.len(), 2);
    for expected in [newer, usd] {
        let quote = quotes.iter().find(|quote| quote.currency == expected.currency).unwrap();
        assert_eq!(quote.price, expected.price);
        assert_eq!(quote.as_of.timestamp_micros(), expected.executed_at.timestamp_micros());
    }
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_amended_execution_retains_runner_up_across_batch_partitions() {
    let event = Utc::now() - ChronoDuration::minutes(10);
    let original = event_report("amended-winner", "unheld", event + ChronoDuration::nanoseconds(200), dec!(100));
    let runner_up = event_report("runner-up", "unheld", event + ChronoDuration::nanoseconds(100), dec!(200));
    let mut amendment = original.clone();
    amendment.executed_at = event;
    amendment.executed_ns = event.timestamp_nanos_opt().unwrap();
    amendment.published_ns += 1000;
    amendment.revision = 1;
    amendment.revision_hash = "amended".into();
    amendment.price = dec!(50);
    let reports = vec![original.clone(), runner_up.clone(), amendment, original.clone()];
    let partitions: &[&[usize]] = &[&[4], &[1, 3], &[2, 2], &[3, 1], &[1, 1, 1, 1]];
    let mut baseline = None;
    for &partition in partitions {
        let (mut db, schema) = test_database().await;
        jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
        let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
        if partition.len() == 1 {
            write_file(&mut db, &job, "revisions", "b", None, reports.clone()).await.unwrap();
        } else {
            let tracked = HashSet::new();
            let scope_hash = price::portfolio_scope_hash(&tracked);
            let mut writer = price::begin_file(&mut db, &job, "revisions", &tracked, &scope_hash).await.unwrap();
            let mut offset = 0;
            for &size in partition {
                writer.push(&reports[offset..offset + size]).await.unwrap();
                offset += size;
            }
            assert_eq!(offset, reports.len());
            writer.finish("b", None).await.unwrap();
        }
        let quotes = price::latest_prices(&db, &["unheld".into()]).await.unwrap();
        assert_eq!(quotes.len(), 1, "partition {partition:?}");
        assert_eq!(quotes[0].price, runner_up.price, "partition {partition:?}");
        assert_eq!(quotes[0].as_of.timestamp_nanos_opt(), Some(runner_up.executed_ns));
        let head = db.query_one(
            "SELECT report_id, price, executed_ns FROM mifid_latest_price", &[],
        ).await.unwrap();
        let snapshot = (head.get::<_, String>(0), head.get::<_, Decimal>(1), head.get::<_, i64>(2));
        assert_eq!(snapshot, (runner_up.id.clone(), runner_up.price, runner_up.executed_ns));
        if let Some(expected) = &baseline {
            assert_eq!(&snapshot, expected, "partition {partition:?}");
        } else {
            baseline = Some(snapshot);
        }
        let counts = db.query_one(
            "SELECT (SELECT count(*) FROM mifid_latest_price), (SELECT count(*) FROM mifid_price_revision),
             (SELECT count(*) FROM market_price_report), (SELECT count(*) FROM instrument)", &[],
        ).await.unwrap();
        assert_eq!(counts.get::<_, i64>(0), 1);
        assert_eq!(counts.get::<_, i64>(1), 1);
        assert_eq!(counts.get::<_, i64>(2), 0);
        assert_eq!(counts.get::<_, i64>(3), 0);
        cleanup(&db, &schema).await;
    }
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_cross_file_backdated_head_stays_unavailable_across_replays_and_reconnect() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let event = Utc::now() - ChronoDuration::minutes(10);
    let winner = event_report("winner", "unheld", event + ChronoDuration::nanoseconds(200), dec!(100));
    let runner = event_report("runner", "unheld", event + ChronoDuration::nanoseconds(100), dec!(200));
    let mut amendment = winner.clone();
    amendment.executed_at = event;
    amendment.executed_ns = event.timestamp_nanos_opt().unwrap();
    amendment.published_ns += 1000;
    amendment.revision = 1;
    amendment.revision_hash = "backdated".into();
    amendment.price = dec!(50);
    write_file(&mut db, &job, "original", "a", None, vec![winner.clone(), runner.clone()]).await.unwrap();
    assert_eq!(price::latest_prices(&db, &["unheld".into()]).await.unwrap()[0].price, dec!(100));
    write_file(&mut db, &job, "amendment", "b", None, vec![amendment.clone()]).await.unwrap();
    assert!(price::latest_prices(&db, &["unheld".into()]).await.unwrap().is_empty());

    // Neither in-memory state nor the importing connection keeps the uncertainty alive.
    drop(db);
    let mut db = connect_schema(&schema).await;
    for (index, rows) in [
        vec![amendment.clone()], vec![winner.clone(), runner.clone()],
        vec![winner.clone(), runner.clone(), amendment.clone()],
        vec![runner], vec![amendment.clone()], vec![winner],
    ].into_iter().enumerate() {
        write_file(&mut db, &job, &format!("replay-{index}"), "r", None, rows).await.unwrap();
        assert!(price::latest_prices(&db, &["unheld".into()]).await.unwrap().is_empty(), "replay {index}");
        jobs::prune(&mut db).await.unwrap();
    }
    let floor = db.query_one(
        "SELECT executed_ns, report_id FROM mifid_price_floor WHERE isin='unheld'", &[],
    ).await.unwrap();
    assert_eq!(floor.get::<_, i64>(0), event.timestamp_nanos_opt().unwrap() + 200);
    assert_eq!(floor.get::<_, String>(1), "winner");
    assert_eq!(db.query_one(
        "SELECT (SELECT count(*) FROM mifid_latest_price), (SELECT count(*) FROM mifid_price_floor),
         (SELECT count(*) FROM market_price_report)", &[],
    ).await.unwrap().get::<_, i64>(0), 0);

    let recent = event_report("recent", "unheld", event + ChronoDuration::nanoseconds(201), dec!(300));
    write_file(&mut db, &job, "recent", "c", None, vec![recent.clone()]).await.unwrap();
    assert_eq!(price::latest_prices(&db, &["unheld".into()]).await.unwrap()[0].price, recent.price);
    let mut correction = recent.clone();
    correction.revision = 1;
    correction.revision_hash = "same-execution".into();
    correction.published_ns += 2000;
    correction.price = dec!(301);
    correction.quantity = Some(Decimal::ZERO);
    write_file(&mut db, &job, "correction", "d", None, vec![correction.clone()]).await.unwrap();
    let quote = price::latest_prices(&db, &["unheld".into()]).await.unwrap().remove(0);
    assert_eq!(quote.price, correction.price);
    assert_eq!(quote.as_of.timestamp_nanos_opt(), Some(recent.executed_ns));
    assert_eq!(quote.quantity, Some(Decimal::ZERO));
    write_file(&mut db, &job, "backdated-replay-after-recovery", "e", None, vec![amendment]).await.unwrap();
    assert_eq!(price::latest_prices(&db, &["unheld".into()]).await.unwrap()[0].price, correction.price);
    let counts = db.query_one(
        "SELECT (SELECT count(*) FROM mifid_latest_price), (SELECT count(*) FROM mifid_price_floor),
         (SELECT count(*) FROM market_price_report)", &[],
    ).await.unwrap();
    assert_eq!(counts.get::<_, i64>(0), 1);
    assert_eq!(counts.get::<_, i64>(1), 0);
    assert_eq!(counts.get::<_, i64>(2), 0);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_later_revision_can_restore_backdated_head_at_original_execution_boundary() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let event = Utc::now() - ChronoDuration::minutes(10);
    let original = event_report("winner", "unheld", event + ChronoDuration::nanoseconds(200), dec!(100));
    let runner = event_report("runner", "unheld", event + ChronoDuration::nanoseconds(100), dec!(200));
    // An earlier file's competitor is lost when the next file publishes W.
    // Re-observing W and R later does not prove R beats this hidden execution.
    let hidden = event_report("hidden", "unheld", event + ChronoDuration::nanoseconds(150), dec!(250));
    write_file(&mut db, &job, "earlier-competitor", "q", None, vec![hidden]).await.unwrap();
    write_file(&mut db, &job, "original", "a", None, vec![original.clone(), runner.clone()]).await.unwrap();
    let mut amendment = original.clone();
    amendment.executed_at = event;
    amendment.executed_ns = event.timestamp_nanos_opt().unwrap();
    amendment.published_ns += 1000;
    amendment.revision = 1;
    amendment.revision_hash = "backdated".into();
    amendment.price = dec!(50);
    write_file(&mut db, &job, "backdated", "b", None, vec![amendment.clone()]).await.unwrap();
    assert!(price::latest_prices(&db, &["unheld".into()]).await.unwrap().is_empty());
    let mut restored = original.clone();
    restored.revision = 1;
    restored.revision_hash = "restored".into();
    restored.published_ns += 2000;
    restored.price = dec!(125);
    write_file(&mut db, &job, "restored", "c", None, vec![restored.clone()]).await.unwrap();
    let quote = price::latest_prices(&db, &["unheld".into()]).await.unwrap().remove(0);
    assert_eq!(quote.price, restored.price);
    assert_eq!(quote.as_of.timestamp_nanos_opt(), Some(original.executed_ns));
    write_file(&mut db, &job, "obsolete-revisions", "d", None, vec![original, amendment]).await.unwrap();
    assert_eq!(price::latest_prices(&db, &["unheld".into()]).await.unwrap()[0].price, restored.price);
    // Even a full replay containing the current persisted head cannot prove its
    // lower runner-up is newer than competitors discarded from earlier files.
    let mut backdated_again = restored.clone();
    backdated_again.executed_at = event;
    backdated_again.executed_ns = event.timestamp_nanos_opt().unwrap();
    backdated_again.published_ns += 1000;
    backdated_again.revision_hash = "backdated-again".into();
    backdated_again.price = dec!(25);
    write_file(&mut db, &job, "full-revision-file", "e", None,
        vec![restored, runner, backdated_again]).await.unwrap();
    assert!(price::latest_prices(&db, &["unheld".into()]).await.unwrap().is_empty());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_cancelled_and_moved_winners_retain_next_heads_across_batch_partitions() {
    let event = Utc::now() - ChronoDuration::minutes(10);
    let cancelled = event_report("cancelled-winner", "cancelled-isin", event + ChronoDuration::seconds(2), dec!(100));
    let cancel_runner = event_report("cancel-runner", "cancelled-isin", event + ChronoDuration::seconds(1), dec!(200));
    let moving = event_report("moving-winner", "old-isin", event + ChronoDuration::seconds(2), dec!(300));
    let move_runner = event_report("move-runner", "old-isin", event + ChronoDuration::seconds(1), dec!(400));
    let mut cancellation = cancelled.clone();
    cancellation.published_ns += 1000;
    cancellation.revision = 2;
    cancellation.revision_hash = "cancelled".into();
    cancellation.eligible = false;
    let mut moved = moving.clone();
    moved.published_ns += 1000;
    moved.revision = 1;
    moved.revision_hash = "moved".into();
    moved.isin = "new-isin".into();
    moved.currency = "USD".into();
    moved.price = dec!(500);
    let reports = vec![
        cancelled.clone(), cancel_runner.clone(), moving.clone(), move_runner.clone(),
        cancellation, moved.clone(), cancelled.clone(), moving.clone(),
    ];
    let partitions: &[&[usize]] = &[&[8], &[4, 4], &[4, 1, 1, 2], &[2, 2, 2, 2], &[1, 1, 1, 1, 1, 1, 1, 1]];
    let mut baseline = None;
    for &partition in partitions {
        let (mut db, schema) = test_database().await;
        jobs::enqueue(&mut db, &[Feed::Cboe]).await.unwrap();
        let job = jobs::claim(&db, &[Feed::Cboe]).await.unwrap().unwrap();
        if partition.len() == 1 {
            write_file(&mut db, &job, "revisions", "b", None, reports.clone()).await.unwrap();
        } else {
            let tracked = HashSet::new();
            let scope_hash = price::portfolio_scope_hash(&tracked);
            let mut writer = price::begin_file(&mut db, &job, "revisions", &tracked, &scope_hash).await.unwrap();
            let mut offset = 0;
            for &size in partition {
                writer.push(&reports[offset..offset + size]).await.unwrap();
                offset += size;
            }
            assert_eq!(offset, reports.len());
            writer.finish("b", None).await.unwrap();
        }
        let requested = vec!["cancelled-isin".into(), "old-isin".into(), "new-isin".into()];
        let quotes = price::latest_prices(&db, &requested).await.unwrap();
        assert_eq!(quotes.len(), 3, "partition {partition:?}");
        for expected in [&cancel_runner, &move_runner, &moved] {
            let quote = quotes.iter().find(|quote| quote.isin == expected.isin).unwrap();
            assert_eq!(quote.currency, expected.currency, "partition {partition:?}");
            assert_eq!(quote.price, expected.price, "partition {partition:?}");
            assert_eq!(quote.as_of.timestamp_nanos_opt(), Some(expected.executed_ns));
        }
        let heads = db.query(
            "SELECT isin, currency, report_id, price FROM mifid_latest_price ORDER BY isin, currency, feed", &[],
        ).await.unwrap();
        let snapshot: Vec<_> = heads.into_iter().map(|head| (
            head.get::<_, String>(0), head.get::<_, String>(1),
            head.get::<_, String>(2), head.get::<_, Decimal>(3),
        )).collect();
        assert_eq!(snapshot, vec![
            ("cancelled-isin".into(), "EUR".into(), cancel_runner.id.clone(), cancel_runner.price),
            ("new-isin".into(), "USD".into(), moved.id.clone(), moved.price),
            ("old-isin".into(), "EUR".into(), move_runner.id.clone(), move_runner.price),
        ], "partition {partition:?}");
        if let Some(expected) = &baseline {
            assert_eq!(&snapshot, expected, "partition {partition:?}");
        } else {
            baseline = Some(snapshot);
        }
        // Replays after the file commits must not resurrect either obsolete identity.
        write_file(&mut db, &job, "old-replay", "c", None, vec![cancelled.clone(), moving.clone()]).await.unwrap();
        let counts = db.query_one(
            "SELECT (SELECT count(*) FROM mifid_latest_price), (SELECT count(*) FROM mifid_price_revision),
             (SELECT count(*) FROM market_price_report), (SELECT count(*) FROM instrument)", &[],
        ).await.unwrap();
        assert_eq!(counts.get::<_, i64>(0), 3);
        assert_eq!(counts.get::<_, i64>(1), 2);
        assert_eq!(counts.get::<_, i64>(2), 0);
        assert_eq!(counts.get::<_, i64>(3), 0);
        assert_eq!(db.query_one(
            "SELECT count(*) FROM mifid_latest_price WHERE report_id=$1 OR (report_id=$2 AND isin=$3)",
            &[&cancelled.id, &moving.id, &moving.isin],
        ).await.unwrap().get::<_, i64>(0), 0);
        cleanup(&db, &schema).await;
    }
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_identity_revisions_move_heads_and_prevent_cancelled_original_resurrection() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Cboe]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Cboe]).await.unwrap().unwrap();
    let event = Utc::now() - ChronoDuration::minutes(5);
    let original = event_report("moving", "old-isin", event, dec!(101));
    write_file(&mut db, &job, "original", "a", None, vec![original.clone()]).await.unwrap();
    let mut amendment = original.clone();
    amendment.isin = "new-isin".into();
    amendment.currency = "JPY".into();
    amendment.price = dec!(15000);
    amendment.revision = 1;
    amendment.published_ns += 1000;
    amendment.revision_hash = "amended".into();
    write_file(&mut db, &job, "amended", "b", None, vec![amendment.clone()]).await.unwrap();
    write_file(&mut db, &job, "replayed-original", "c", None, vec![original]).await.unwrap();
    let requested = vec!["old-isin".into(), "new-isin".into(), "cancelled".into()];
    let quotes = price::latest_prices(&db, &requested).await.unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].isin, "new-isin");
    assert_eq!(quotes[0].currency, "JPY");
    assert_eq!(quotes[0].price, dec!(15000));
    let unseen = event_report("unseen-cancel", "cancelled", event, dec!(999));
    let mut cancellation = unseen.clone();
    cancellation.revision = 2;
    cancellation.eligible = false;
    cancellation.published_ns += 1000;
    let tracked = HashSet::new();
    let scope_hash = price::portfolio_scope_hash(&tracked);
    let mut writer = price::begin_file(&mut db, &job, "cancel-first", &tracked, &scope_hash).await.unwrap();
    writer.push(&[cancellation]).await.unwrap();
    writer.push(std::slice::from_ref(&unseen)).await.unwrap();
    writer.finish("d", None).await.unwrap();
    write_file(&mut db, &job, "cancelled-replay", "e", None, vec![unseen]).await.unwrap();
    let mut cancel_moved = amendment;
    cancel_moved.revision = 2;
    cancel_moved.eligible = false;
    cancel_moved.published_ns += 1000;
    write_file(&mut db, &job, "cancel-moved", "f", None, vec![cancel_moved]).await.unwrap();
    assert!(price::latest_prices(&db, &requested).await.unwrap().is_empty());
    assert_eq!(db.query_one("SELECT count(*) FROM market_price_report", &[]).await.unwrap().get::<_, i64>(0), 0);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_unheld_trade_updates_keep_only_bounded_latest_heads() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Tradegate]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Tradegate]).await.unwrap().unwrap();
    let event = Utc::now() - ChronoDuration::minutes(5);
    let rows: Vec<_> = (0..10003).map(|index| {
        event_report(&format!("trade-{index}"), "unheld", event + ChronoDuration::microseconds(index), Decimal::from(index + 1))
    }).collect();
    write_file(&mut db, &job, "many-updates", "a", None, rows).await.unwrap();
    let quotes = price::latest_prices(&db, &["unheld".into()]).await.unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].price, dec!(10003));
    let counts = db.query_one(
        "SELECT (SELECT count(*) FROM mifid_latest_price), (SELECT count(*) FROM mifid_price_revision),
         (SELECT count(*) FROM market_price_report), (SELECT count(*) FROM instrument)", &[],
    ).await.unwrap();
    assert_eq!(counts.get::<_, i64>(0), 1);
    for index in 1..4 {
        assert_eq!(counts.get::<_, i64>(index), 0);
    }
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_replay_hydrates_legacy_indicative_quantity_without_renewing_event() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Tradegate]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Tradegate]).await.unwrap().unwrap();
    let event = Utc::now() - ChronoDuration::minutes(20);
    let mut original = event_report("legacy-quantity", "unheld", event, dec!(42));
    original.quantity = Some(Decimal::ZERO);
    write_file(&mut db, &job, "original", "a", None, vec![original.clone()]).await.unwrap();
    // Pre-V25 reports migrated into latest heads have no observed quantity.
    db.execute("UPDATE mifid_latest_price SET quantity=NULL WHERE report_id=$1", &[&original.id])
        .await.unwrap();
    write_file(&mut db, &job, "metadata-replay", "a", None, vec![original]).await.unwrap();
    let reference = price::latest_prices(&db, &["unheld".into()]).await.unwrap().remove(0);
    assert_eq!(reference.quantity, Some(Decimal::ZERO));
    assert_eq!(reference.price, dec!(42));
    assert_eq!(reference.as_of.timestamp_nanos_opt(), event.timestamp_nanos_opt());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_bulk_finish_failure_rolls_back_heads_revision_guards_checkpoint_and_progress() {
    let (mut db, schema) = test_database().await;
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    let original = event_report("rollback", "rollback-held", Utc::now() - ChronoDuration::minutes(5), dec!(123));
    let tracked = HashSet::from([original.isin.clone()]);
    price::ensure_instruments(&db, &tracked).await.unwrap();
    write_file(&mut db, &job, "original", "a", None, vec![original.clone()]).await.unwrap();
    db.batch_execute(
        "CREATE FUNCTION reject_bulk_checkpoint() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'checkpoint rejected'; END $$;
         CREATE TRIGGER reject_bulk_checkpoint BEFORE INSERT ON market_data_file
             FOR EACH ROW EXECUTE FUNCTION reject_bulk_checkpoint()",
    ).await.unwrap();
    let mut row = original.clone();
    row.revision = 1;
    row.revision_hash = "backdated".into();
    row.published_ns += 1000;
    row.executed_ns -= 1;
    row.executed_at -= ChronoDuration::nanoseconds(1);
    row.price = dec!(50);
    let scope_hash = price::portfolio_scope_hash(&tracked);
    let mut writer = price::begin_file(&mut db, &job, "failed", &tracked, &scope_hash).await.unwrap();
    writer.push(&[row]).await.unwrap();
    assert!(writer.finish("hash", None).await.is_err());
    let quote = price::latest_prices(&db, &[original.isin.clone()]).await.unwrap().remove(0);
    assert_eq!(quote.price, original.price);
    assert_eq!(quote.as_of.timestamp_nanos_opt(), Some(original.executed_ns));
    let source = db.query_one(
        "SELECT revision, price FROM market_price_report WHERE id=$1", &[&original.id],
    ).await.unwrap();
    assert_eq!(source.get::<_, i32>(0), original.revision);
    assert_eq!(source.get::<_, Decimal>(1), original.price);
    assert_eq!(price::quotes(&db, &[original.isin.clone()]).await.unwrap()[0].price, Some(original.price));
    let state = db.query_one(
        "SELECT (SELECT count(*) FROM mifid_latest_price), (SELECT count(*) FROM mifid_price_revision),
         (SELECT count(*) FROM market_data_file), (SELECT count(*) FROM mifid_price_floor),
         files_processed, reports_processed FROM background_job WHERE id=$1", &[&job.id],
    ).await.unwrap();
    assert_eq!(state.get::<_, i64>(0), 1);
    assert_eq!(state.get::<_, i64>(1), 0);
    assert_eq!(state.get::<_, i64>(2), 1);
    assert_eq!(state.get::<_, i64>(3), 0);
    assert_eq!(state.get::<_, i32>(4), 1);
    assert_eq!(state.get::<_, i32>(5), 1);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn pricing_database_recency_floor_upgrade_withdraws_unproven_revision_heads_until_new_execution() {
    let (mut db, schema) = test_database_at(refinery::Target::Version(31)).await;
    let event = Utc::now() - ChronoDuration::minutes(10);
    let original = event_report("preserved-original", "preserved", event, dec!(100));
    let mut amendment = event_report("legacy-amendment", "uncertain", event, dec!(50));
    amendment.revision = 1;
    amendment.published_ns += 1000;
    amendment.revision_hash = "legacy".into();
    for row in [&original, &amendment] {
        db.execute(
            "INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price,
                 as_of, executed_ns, published_ns, revision, revision_hash)
             VALUES($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            &[&row.isin, &row.currency, &Feed::Xetra.key(), &row.id, &row.venue,
                &row.price, &row.executed_at, &row.executed_ns, &row.published_ns,
                &row.revision, &row.revision_hash],
        ).await.unwrap();
    }
    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    let requested = ["preserved".into(), "uncertain".into()];
    let quotes = price::latest_prices(&db, &requested).await.unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].isin, original.isin);
    assert_eq!(quotes[0].price, original.price);
    let floor: i64 = db.query_one(
        "SELECT executed_ns FROM mifid_price_floor WHERE isin='uncertain'", &[],
    ).await.unwrap().get(0);
    assert!(floor > amendment.executed_ns);
    assert!(migrations::runner().run_async(&mut db).await.unwrap().applied_migrations().is_empty());
    jobs::enqueue(&mut db, &[Feed::Xetra]).await.unwrap();
    let job = jobs::claim(&db, &[Feed::Xetra]).await.unwrap().unwrap();
    write_file(&mut db, &job, "legacy-replay", "a", None, vec![amendment.clone()]).await.unwrap();
    amendment.published_ns += 1000;
    amendment.revision_hash = "legacy-price-correction".into();
    amendment.price = dec!(75);
    write_file(&mut db, &job, "legacy-correction", "b", None, vec![amendment]).await.unwrap();
    assert!(price::latest_prices(&db, &["uncertain".into()]).await.unwrap().is_empty());
    let fresh = event_report("post-upgrade", "uncertain", Utc::now(), dec!(200));
    write_file(&mut db, &job, "post-upgrade", "c", None, vec![fresh.clone()]).await.unwrap();
    let quotes = price::latest_prices(&db, &["uncertain".into()]).await.unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].price, fresh.price);
    assert_eq!(quotes[0].as_of.timestamp_nanos_opt(), Some(fresh.executed_ns));
    cleanup(&db, &schema).await;
}
