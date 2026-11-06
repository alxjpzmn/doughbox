use std::collections::HashMap;

use tokio_postgres::{Client, NoTls};

use super::*;

async fn test_database() -> (Client, String) {
    // Never use POSTGRES_URL or load a developer's .env for database tests.
    let url = std::env::var("TEST_POSTGRES_URL")
        .expect("Set TEST_POSTGRES_URL to an isolated PostgreSQL instance");
    let (mut db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move { connection.await.unwrap() });
    let schema = format!("trading_test_{}", uuid::Uuid::new_v4().simple());
    db.batch_execute(&format!(
        "CREATE SCHEMA {schema}; SET search_path TO {schema}"
    ))
    .await
    .unwrap();
    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    (db, schema)
}

async fn connect_schema(schema: &str) -> Client {
    let url = std::env::var("TEST_POSTGRES_URL").unwrap();
    let (db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move { connection.await.unwrap() });
    db.batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    db
}

async fn cleanup(db: &Client, schema: &str) {
    db.batch_execute(&format!(
        "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
    ))
    .await
    .unwrap();
}

fn trade(status: &str, broker_order_id: Option<&str>, quantity: &str) -> TradeResult {
    TradeResult {
        trade: TradeInput {
            ticker: "AAPL_US_EQ".into(),
            side: "BUY".into(),
            quantity: quantity.into(),
            limit_price: "247.123456789012345678".into(),
            time_validity: "GOOD_TILL_CANCEL".into(),
        },
        name: "Apple".into(),
        currency: "USD".into(),
        status: status.into(),
        broker_order_id: broker_order_id.map(str::to_owned),
        error: Some("previous broker message".into()),
    }
}

async fn insert_proposal(db: &Client, status: &str, trades: &[TradeResult]) {
    db.execute(
        "INSERT INTO trading_proposals
         (id, generation, client_request_id, request, title, note, environment, status, trades)
         VALUES ('proposal', 'test', 'request', '{}', 'Approved trades', '', 'demo', $1, $2)",
        &[&status, &serde_json::to_value(trades).unwrap()],
    )
    .await
    .unwrap();
}

async fn load_proposal(db: &Client) -> Proposal {
    let row = db
        .query_one("SELECT * FROM trading_proposals WHERE id='proposal'", &[])
        .await
        .unwrap();
    Proposal::from_row(&row).unwrap()
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_stale_poll_preserves_later_accepted_leg_and_approved_values() {
    let (db, schema) = test_database().await;
    let executor = connect_schema(&schema).await;
    let initial = vec![
        trade("submitted", Some("101"), "1.23456789"),
        trade("pending", None, "0.00000001"),
    ];
    insert_proposal(&db, "executing", &initial).await;

    // state() read this before awaiting broker HTTP; execute() then accepted leg two.
    let mut polled = load_proposal(&db).await;
    assert_eq!(polled.trades[1].status, "pending");
    assert_eq!(polled.trades[1].broker_order_id, None);
    let mut executed = initial.clone();
    executed[1].status = "submitted".into();
    executed[1].broker_order_id = Some("102".into());
    executed[1].error = None;
    store::save_trades(&executor, &polled.id, &executed)
        .await
        .unwrap();
    executor
        .execute("UPDATE trading_proposals SET status='completed' WHERE id=$1", &[&polled.id])
        .await
        .unwrap();

    let updates = HashMap::from([("101".into(), "open".into())]);
    polled = store::reconcile_order_statuses(&db, &polled.id, &updates)
        .await
        .unwrap();
    assert_eq!(polled.status, "completed");
    assert_eq!(polled.trades[0].status, "open");
    assert_eq!(polled.trades[0].error, None);
    assert_eq!(polled.trades[1].status, "submitted");
    assert_eq!(polled.trades[1].broker_order_id.as_deref(), Some("102"));
    for (actual, approved) in polled.trades.iter().zip(&initial) {
        assert_eq!(actual.trade, approved.trade);
        assert_eq!(actual.name, approved.name);
        assert_eq!(actual.currency, approved.currency);
    }
    assert_eq!(
        serde_json::to_value(&polled.trades[1]).unwrap(),
        serde_json::to_value(&executed[1]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&polled).unwrap(),
        serde_json::to_value(load_proposal(&db).await).unwrap()
    );
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_poll_cannot_change_executing_proposal() {
    let (db, schema) = test_database().await;
    let trades = vec![
        trade("submitted", Some("101"), "1.23456789"),
        trade("submitting", None, "0.00000001"),
    ];
    insert_proposal(&db, "executing", &trades).await;
    let before = serde_json::to_value(load_proposal(&db).await).unwrap();
    let updates = HashMap::from([("101".into(), "filled".into())]);
    let mut returned = store::reconcile_order_statuses(&db, "proposal", &updates)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&returned).unwrap(), before);
    assert_eq!(serde_json::to_value(load_proposal(&db).await).unwrap(), before);

    // An executing-only state must not claim a poll or decrypt credentials/call HTTP.
    reconcile_orders(&db, "demo", "not encrypted credentials", std::slice::from_mut(&mut returned))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&returned).unwrap(), before);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_poll_matches_current_ids_and_never_regresses_terminal_or_unsent_legs() {
    let (db, schema) = test_database().await;
    let trades = vec![
        trade("submitted", Some("101"), "1"),
        trade("open", Some("102"), "2"),
        trade("partial", Some("103"), "3"),
        trade("pending", Some("104"), "4"),
        trade("submitting", Some("105"), "5"),
        trade("unknown", Some("106"), "6"),
        trade("failed", Some("107"), "7"),
        trade("discarded", Some("108"), "8"),
    ];
    insert_proposal(&db, "completed", &trades).await;
    let terminal = HashMap::from([
        ("101".into(), "filled".into()),
        ("102".into(), "cancelled".into()),
    ]);
    let current = store::reconcile_order_statuses(&db, "proposal", &terminal)
        .await
        .unwrap();
    assert_eq!(current.trades[0].status, "filled");
    assert_eq!(current.trades[1].status, "cancelled");
    let stale = (101..=108).map(|id| (id.to_string(), "open".into())).collect();
    let current = store::reconcile_order_statuses(&db, "proposal", &stale)
        .await
        .unwrap();
    assert_eq!(current.trades[0].status, "filled");
    assert_eq!(current.trades[1].status, "cancelled");
    assert_eq!(current.trades[2].status, "open");
    assert_eq!(current.trades[2].error, None);
    for index in 3..trades.len() {
        assert_eq!(
            serde_json::to_value(&current.trades[index]).unwrap(),
            serde_json::to_value(&trades[index]).unwrap()
        );
    }

    // A delayed response for the old ID must not apply to the current order.
    db.execute(
        "UPDATE trading_proposals SET trades=jsonb_set(trades, '{2,broker_order_id}', '\"203\"') WHERE id='proposal'",
        &[],
    )
    .await
    .unwrap();
    let stale = HashMap::from([("103".into(), "filled".into())]);
    let current = store::reconcile_order_statuses(&db, "proposal", &stale)
        .await
        .unwrap();
    assert_eq!(current.trades[2].broker_order_id.as_deref(), Some("203"));
    assert_eq!(current.trades[2].status, "open");
    let matching = HashMap::from([("203".into(), "partial".into())]);
    let current = store::reconcile_order_statuses(&db, "proposal", &matching)
        .await
        .unwrap();
    assert_eq!(current.trades[2].status, "partial");
    assert_eq!(
        serde_json::to_value(&current).unwrap(),
        serde_json::to_value(load_proposal(&db).await).unwrap()
    );
    cleanup(&db, &schema).await;
}
