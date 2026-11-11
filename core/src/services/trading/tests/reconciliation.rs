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

async fn owner(db: &Client, username: &str) -> UserId {
    let user_id = UserId::new();
    db.execute(
        "INSERT INTO auth_users (id,username,password_hash,role,status,totp_secret_encrypted) VALUES ($1,$2,'fixture-password','member','active','fixture-secret')",
        &[&user_id.as_str(), &username],
    ).await.unwrap();
    db.execute(
        "INSERT INTO trading_connection (user_id,generation,environment,credentials) VALUES ($1,'test','demo','fixture-credentials')",
        &[&user_id.as_str()],
    ).await.unwrap();
    user_id
}

pub(super) async fn connect_schema(schema: &str) -> Client {
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

async fn insert_proposal(user_id: &UserId, db: &Client, status: &str, trades: &[TradeResult]) {
    db.execute(
        "INSERT INTO trading_proposals
         (user_id, id, generation, client_request_id, request, title, note, environment, status, trades)
         VALUES ($1, $1, 'test', 'request', '{}', 'Approved trades', '', 'demo', $2, $3)",
        &[&user_id.as_str(), &status, &serde_json::to_value(trades).unwrap()],
    )
    .await
    .unwrap();
}

async fn load_proposal(user_id: &UserId, db: &Client) -> Proposal {
    let row = db
        .query_one("SELECT * FROM trading_proposals WHERE user_id=$1 AND id=$1", &[&user_id.as_str()])
        .await
        .unwrap();
    Proposal::from_row(&row).unwrap()
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_stale_poll_preserves_later_accepted_leg_and_approved_values() {
    let (db, schema) = test_database().await;
    let user_id = owner(&db, "fixture").await;
    let executor = connect_schema(&schema).await;
    let initial = vec![
        trade("submitted", Some("101"), "1.23456789"),
        trade("pending", None, "0.00000001"),
    ];
    insert_proposal(&user_id, &db, "executing", &initial).await;

    // state() read this before awaiting broker HTTP; execute() then accepted leg two.
    let mut polled = load_proposal(&user_id, &db).await;
    let ProposalContent::Trades { trades: polled_trades } = &polled.content else { panic!("expected Trades") };
    assert_eq!(polled_trades[1].status, "pending");
    assert_eq!(polled_trades[1].broker_order_id, None);
    let mut executed = initial.clone();
    executed[1].status = "submitted".into();
    executed[1].broker_order_id = Some("102".into());
    executed[1].error = None;
    store::save_trades(&user_id, &executor, &polled.id, &executed)
        .await
        .unwrap();
    executor
        .execute("UPDATE trading_proposals SET status='completed' WHERE user_id=$1 AND id=$2", &[&user_id.as_str(), &polled.id])
        .await
        .unwrap();

    let updates = HashMap::from([("101".into(), "open".into())]);
    polled = store::reconcile_order_statuses(&user_id, &db, &polled.id, &updates)
        .await
        .unwrap();
    assert_eq!(polled.status, "completed");
    let ProposalContent::Trades { trades: polled_trades } = &polled.content else { panic!("expected Trades") };
    assert_eq!(polled_trades[0].status, "open");
    assert_eq!(polled_trades[0].error, None);
    assert_eq!(polled_trades[1].status, "submitted");
    assert_eq!(polled_trades[1].broker_order_id.as_deref(), Some("102"));
    for (actual, approved) in polled_trades.iter().zip(&initial) {
        assert_eq!(actual.trade, approved.trade);
        assert_eq!(actual.name, approved.name);
        assert_eq!(actual.currency, approved.currency);
    }
    assert_eq!(
        serde_json::to_value(&polled_trades[1]).unwrap(),
        serde_json::to_value(&executed[1]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&polled).unwrap(),
        serde_json::to_value(load_proposal(&user_id, &db).await).unwrap()
    );
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_poll_cannot_change_executing_proposal() {
    let (db, schema) = test_database().await;
    let user_id = owner(&db, "fixture").await;
    let trades = vec![
        trade("submitted", Some("101"), "1.23456789"),
        trade("submitting", None, "0.00000001"),
    ];
    insert_proposal(&user_id, &db, "executing", &trades).await;
    let before = serde_json::to_value(load_proposal(&user_id, &db).await).unwrap();
    let updates = HashMap::from([("101".into(), "filled".into())]);
    let mut returned = store::reconcile_order_statuses(&user_id, &db, user_id.as_str(), &updates)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&returned).unwrap(), before);
    assert_eq!(serde_json::to_value(load_proposal(&user_id, &db).await).unwrap(), before);

    // An executing-only state must not claim a poll or decrypt credentials/call HTTP.
    reconcile_orders(&user_id, &db, "demo", "not encrypted credentials", std::slice::from_mut(&mut returned))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&returned).unwrap(), before);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_poll_matches_current_ids_and_never_regresses_terminal_or_unsent_legs() {
    let (db, schema) = test_database().await;
    let user_id = owner(&db, "fixture").await;
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
    insert_proposal(&user_id, &db, "completed", &trades).await;
    let terminal = HashMap::from([
        ("101".into(), "filled".into()),
        ("102".into(), "cancelled".into()),
    ]);
    let current = store::reconcile_order_statuses(&user_id, &db, user_id.as_str(), &terminal)
        .await
        .unwrap();
    let ProposalContent::Trades { trades: current_trades } = &current.content else { panic!("expected Trades") };
    assert_eq!(current_trades[0].status, "filled");
    assert_eq!(current_trades[1].status, "cancelled");
    let stale = (101..=108).map(|id| (id.to_string(), "open".into())).collect();
    let current = store::reconcile_order_statuses(&user_id, &db, user_id.as_str(), &stale)
        .await
        .unwrap();
    let ProposalContent::Trades { trades: current_trades } = &current.content else { panic!("expected Trades") };
    assert_eq!(current_trades[0].status, "filled");
    assert_eq!(current_trades[1].status, "cancelled");
    assert_eq!(current_trades[2].status, "open");
    assert_eq!(current_trades[2].error, None);
    for index in 3..trades.len() {
        assert_eq!(
            serde_json::to_value(&current_trades[index]).unwrap(),
            serde_json::to_value(&trades[index]).unwrap()
        );
    }

    // A delayed response for the old ID must not apply to the current order.
    db.execute(
        "UPDATE trading_proposals SET trades=jsonb_set(trades, '{2,broker_order_id}', '\"203\"') WHERE user_id=$1 AND id=$1",
        &[&user_id.as_str()],
    )
    .await
    .unwrap();
    let stale = HashMap::from([("103".into(), "filled".into())]);
    let current = store::reconcile_order_statuses(&user_id, &db, user_id.as_str(), &stale)
        .await
        .unwrap();
    let ProposalContent::Trades { trades: current_trades } = &current.content else { panic!("expected Trades") };
    assert_eq!(current_trades[2].broker_order_id.as_deref(), Some("203"));
    assert_eq!(current_trades[2].status, "open");
    let matching = HashMap::from([("203".into(), "partial".into())]);
    let current = store::reconcile_order_statuses(&user_id, &db, user_id.as_str(), &matching)
        .await
        .unwrap();
    let ProposalContent::Trades { trades: current_trades } = &current.content else { panic!("expected Trades") };
    assert_eq!(current_trades[2].status, "partial");
    assert_eq!(
        serde_json::to_value(&current).unwrap(),
        serde_json::to_value(load_proposal(&user_id, &db).await).unwrap()
    );
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn tenant_isolation_database_trading_execution_recovery_and_reconnect_are_owner_local() {
    let (mut db, schema) = test_database().await;
    let other = connect_schema(&schema).await;
    let alice = owner(&db, "alice").await;
    let bob = owner(&db, "bob").await;
    let trades = vec![trade("submitting", None, "1"), trade("pending", None, "2")];
    insert_proposal(&alice, &db, "executing", &trades).await;
    insert_proposal(&bob, &db, "executing", &trades).await;
    db.execute(
        "INSERT INTO trading_proposals (user_id,id,generation,client_request_id,request,title,note,environment,status,trades)
         SELECT user_id,'bob-pending',generation,'another-request',request,title,note,environment,'pending',trades
         FROM trading_proposals WHERE user_id=$1",
        &[&bob.as_str()],
    ).await.unwrap();
    let bob_proposals: Value = db.query_one(
        "SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM trading_proposals p WHERE user_id=$1",
        &[&bob.as_str()],
    ).await.unwrap().get(0);
    let bob_connection: Value = db.query_one(
        "SELECT to_jsonb(c) FROM trading_connection c WHERE user_id=$1", &[&bob.as_str()],
    ).await.unwrap().get(0);

    assert!(store::execution_lock(&alice, &db).await.unwrap());
    assert!(!store::execution_lock(&alice, &other).await.unwrap());
    assert!(store::execution_lock(&bob, &other).await.unwrap());
    store::recover_on(&alice, &db).await.unwrap();
    let recovered = load_proposal(&alice, &db).await;
    assert_eq!(recovered.status, "completed");
    let ProposalContent::Trades { trades: recovered_trades } = &recovered.content else { panic!("expected Trades") };
    assert_eq!(recovered_trades[0].status, "unknown");
    assert_eq!(recovered_trades[1].status, "discarded");
    assert_eq!(load_proposal(&bob, &db).await.status, "executing");

    // Reconnect A while B's execution lock is held, without touching B's proposals.
    db.execute("UPDATE trading_proposals SET status='pending' WHERE user_id=$1", &[&alice.as_str()]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    store::replace_connection_on(&alice, &tx, None).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(load_proposal(&alice, &db).await.status, "discarded");
    store::mark_submission(&alice, &db).await.unwrap();
    assert!(store::claim_poll(&alice, &db, "last_pending_poll_at", "6 seconds").await.unwrap());
    assert!(!store::claim_poll(&alice, &db, "last_pending_poll_at", "6 seconds").await.unwrap());
    let after_connection: Value = db.query_one(
        "SELECT to_jsonb(c) FROM trading_connection c WHERE user_id=$1", &[&bob.as_str()],
    ).await.unwrap().get(0);
    assert_eq!(after_connection, bob_connection);
    let after_proposals: Value = db.query_one(
        "SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM trading_proposals p WHERE user_id=$1",
        &[&bob.as_str()],
    ).await.unwrap().get(0);
    assert_eq!(after_proposals, bob_proposals);
    let unknown = store::reconcile_order_statuses(&alice, &db, bob.as_str(), &HashMap::new()).await;
    assert!(matches!(unknown.err().unwrap().downcast_ref::<AuthError>(), Some(AuthError::NotFound)));
    assert!(store::claim_poll(&bob, &other, "last_pending_poll_at", "6 seconds").await.unwrap());
    // Interrupted execution can still be safely finalized after account disable.
    other.execute("UPDATE auth_users SET status='disabled' WHERE id=$1", &[&bob.as_str()]).await.unwrap();
    store::recover_on(&bob, &other).await.unwrap();
    assert_eq!(load_proposal(&bob, &db).await.status, "completed");
    drop(other);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn tenant_isolation_database_trading_agent_checks_owner_generation_and_account_authority() {
    let (mut db, schema) = test_database().await;
    let alice = owner(&db, "alice").await;
    let bob = owner(&db, "bob").await;
    insert_proposal(&alice, &db, "pending", &[trade("pending", None, "1")]).await;
    insert_proposal(&bob, &db, "pending", &[trade("pending", None, "1")]).await;
    db.execute("UPDATE trading_connection SET agent_token_hash='alice-hash',agent_auth_version=1 WHERE user_id=$1", &[&alice.as_str()]).await.unwrap();
    db.execute("UPDATE trading_connection SET agent_token_hash='bob-hash',agent_auth_version=1 WHERE user_id=$1", &[&bob.as_str()]).await.unwrap();
    assert_eq!(store::authenticate_on(&db, "alice-hash").await.unwrap(), alice);
    assert_eq!(store::authenticate_on(&db, "bob-hash").await.unwrap(), bob);
    let tx = db.transaction().await.unwrap();
    assert!(store::agent_connection(&bob, &tx, "alice-hash").await.is_err());
    tx.rollback().await.unwrap();
    let tx = db.transaction().await.unwrap();
    let missing = store::agent_proposal(&alice, &tx, "alice-hash", bob.as_str()).await;
    assert!(matches!(missing.err().unwrap().downcast_ref::<AuthError>(), Some(AuthError::NotFound)));
    tx.rollback().await.unwrap();
    let tx = db.transaction().await.unwrap();
    assert_eq!(store::agent_proposal(&alice, &tx, "alice-hash", alice.as_str()).await.unwrap().id, alice.as_str());
    tx.rollback().await.unwrap();

    // A changed generation cannot adopt an older proposal, even with a valid token.
    db.execute("UPDATE trading_connection SET generation='replacement' WHERE user_id=$1", &[&alice.as_str()]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    let old = store::agent_proposal(&alice, &tx, "alice-hash", alice.as_str()).await;
    assert!(matches!(old.err().unwrap().downcast_ref::<AuthError>(), Some(AuthError::NotFound)));
    tx.rollback().await.unwrap();
    // Deliberately retain the hash: the captured version independently denies it.
    db.execute("UPDATE auth_users SET auth_version=auth_version+1 WHERE id=$1", &[&alice.as_str()]).await.unwrap();
    assert!(store::authenticate_on(&db, "alice-hash").await.is_err());
    let tx = db.transaction().await.unwrap();
    assert!(store::agent_connection(&alice, &tx, "alice-hash").await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(store::authenticate_on(&db, "bob-hash").await.unwrap(), bob);
    db.execute("UPDATE auth_users SET status='disabled' WHERE id=$1", &[&bob.as_str()]).await.unwrap();
    assert!(store::authenticate_on(&db, "bob-hash").await.is_err());
    let tx = db.transaction().await.unwrap();
    assert!(store::agent_connection(&bob, &tx, "bob-hash").await.is_err());
    tx.rollback().await.unwrap();
    cleanup(&db, &schema).await;
}

async fn browser_source(user_id: &UserId, db: &mut Client, now: DateTime<Utc>) -> (CurrentUser, AuthSession) {
    use crate::{database::models::auth::CredentialKind, services::auth::session_store::{self, SessionGrant}};
    let user = crate::database::queries::auth::by_id(user_id, db).await.unwrap().unwrap();
    let actor = user.current(CredentialKind::BrowserSession).unwrap();
    let tx = db.transaction().await.unwrap();
    let anonymous = session_store::create_anonymous(&tx, now).await.unwrap();
    let full = session_store::issue_successor_locked(&tx, &anonymous.session, SessionGrant::full(&user, now), now).await.unwrap();
    tx.commit().await.unwrap();
    (actor, full.session)
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_token_issuance_rechecks_source_recency_and_browser_credential() {
    use crate::database::models::auth::CredentialKind;
    let (mut db, schema) = test_database().await;
    let user_id = owner(&db, "alice").await;
    let now = Utc::now();
    let (actor, source) = browser_source(&user_id, &mut db, now).await;
    let tx = db.transaction().await.unwrap();
    store::rotate_token_on(&user_id, &tx, &actor, &source, "issued-hash", now).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(store::authenticate_on(&db, "issued-hash").await.unwrap(), user_id);
    let tx = db.transaction().await.unwrap();
    let old = store::rotate_token_on(&user_id, &tx, &actor, &source, "stale-hash", now + chrono::Duration::minutes(6)).await;
    assert!(matches!(old, Err(AuthError::ReauthenticationRequired)));
    tx.rollback().await.unwrap();
    let mut automation = actor.clone();
    automation.credential = CredentialKind::PersonalWriteToken;
    let tx = db.transaction().await.unwrap();
    assert!(matches!(store::rotate_token_on(&user_id, &tx, &automation, &source, "automation-hash", now).await, Err(AuthError::Forbidden)));
    tx.rollback().await.unwrap();
    db.execute("UPDATE auth_sessions SET revision=revision+1 WHERE id_hash=$1", &[&source.id_hash]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    assert!(matches!(store::rotate_token_on(&user_id, &tx, &actor, &source, "stale-source", now).await, Err(AuthError::StateChanged)));
    tx.rollback().await.unwrap();
    assert_eq!(store::authenticate_on(&db, "issued-hash").await.unwrap(), user_id);
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn trading_database_source_revocation_wins_against_inflight_agent_token_issuance() {
    use crate::{database::queries::auth, services::auth::session_store};
    let (mut db, schema) = test_database().await;
    let mut issuer = connect_schema(&schema).await;
    let user_id = owner(&db, "alice").await;
    let now = Utc::now();
    let (actor, source) = browser_source(&user_id, &mut db, now).await;
    let tx = db.transaction().await.unwrap();
    auth::lock_actor(&tx, &actor, false).await.unwrap();
    session_store::lock_source(&tx, &source, now).await.unwrap();
    tx.execute("DELETE FROM auth_sessions WHERE id_hash=$1", &[&source.id_hash]).await.unwrap();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let owner = user_id.clone();
    let inflight = tokio::spawn(async move {
        let tx = issuer.transaction().await.unwrap();
        started.send(()).unwrap();
        let result = store::rotate_token_on(&owner, &tx, &actor, &source, "must-not-exist", now).await;
        tx.rollback().await.unwrap();
        result
    });
    waiting.await.unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(inflight.await.unwrap(), Err(AuthError::StateChanged)));
    let hash: Option<String> = db.query_one("SELECT agent_token_hash FROM trading_connection WHERE user_id=$1", &[&user_id.as_str()]).await.unwrap().get(0);
    assert!(hash.is_none());
    cleanup(&db, &schema).await;
}

fn submission() -> SubmitInput {
    SubmitInput {
        client_request_id: "same-request".into(),
        title: "Review".into(),
        note: String::new(),
        trades: vec![TradeInput {
            ticker: "AAPL_US_EQ".into(),
            side: "buy".into(),
            quantity: "1".into(),
            limit_price: "10".into(),
            time_validity: "DAY".into(),
        }],
    }
}

fn submission_catalog(generation: &str) -> catalog::Catalog {
    catalog::Catalog {
        generation: generation.into(),
        instruments: vec![Instrument {
            ticker: "AAPL_US_EQ".into(),
            name: "Apple".into(),
            currency: "USD".into(),
            isin: "US0378331005".into(),
        }],
    }
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn tenant_isolation_database_trading_concurrent_replay_is_local_and_reconnect_cannot_reuse_catalog() {
    let (mut db, schema) = test_database().await;
    let mut other = connect_schema(&schema).await;
    let alice = owner(&db, "alice").await;
    let bob = owner(&db, "bob").await;
    db.execute("UPDATE trading_connection SET agent_token_hash='alice',agent_auth_version=1 WHERE user_id=$1", &[&alice.as_str()]).await.unwrap();
    db.execute("UPDATE trading_connection SET agent_token_hash='bob',agent_auth_version=1 WHERE user_id=$1", &[&bob.as_str()]).await.unwrap();
    let (first, replay) = tokio::join!(
        async {
            let tx = db.transaction().await.unwrap();
            let outcome = submit_on(&alice, &tx, "alice", submission(), submission_catalog("test")).await.unwrap();
            tx.commit().await.unwrap();
            outcome
        },
        async {
            let tx = other.transaction().await.unwrap();
            let outcome = submit_on(&alice, &tx, "alice", submission(), submission_catalog("test")).await.unwrap();
            tx.commit().await.unwrap();
            outcome
        }
    );
    assert_ne!(first.created, replay.created);
    assert_eq!(first.proposal.id, replay.proposal.id);
    let tx = db.transaction().await.unwrap();
    let second_owner = submit_on(&bob, &tx, "bob", submission(), submission_catalog("test")).await.unwrap();
    tx.commit().await.unwrap();
    assert!(second_owner.created);
    assert_ne!(second_owner.proposal.id, first.proposal.id);
    let tx = db.transaction().await.unwrap();
    let mut changed = submission();
    changed.trades[0].quantity = "2".into();
    assert!(submit_on(&alice, &tx, "alice", changed, submission_catalog("test")).await.is_err());
    tx.rollback().await.unwrap();
    db.execute("UPDATE trading_connection SET generation='replacement' WHERE user_id=$1", &[&alice.as_str()]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    assert!(submit_on(&alice, &tx, "alice", submission(), submission_catalog("test")).await.is_err());
    tx.rollback().await.unwrap();
    let counts: Vec<(String, i64)> = db.query(
        "SELECT user_id,count(*) FROM trading_proposals GROUP BY user_id ORDER BY user_id", &[],
    ).await.unwrap().into_iter().map(|row| (row.get(0), row.get(1))).collect();
    assert_eq!(counts.len(), 2);
    assert!(counts.iter().all(|(_, count)| *count == 1));
    drop(other);
    cleanup(&db, &schema).await;
}
