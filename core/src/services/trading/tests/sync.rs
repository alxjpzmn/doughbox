//! Real PostgreSQL and broker scenarios; never run against a portfolio database.
use super::*;
use crate::database::queries::{auth, trading212_import};
use serde_json::json;
use tokio_postgres::NoTls;

#[path = "sync_ingestion.rs"]
mod ingestion;
#[path = "sync_budget.rs"]
mod budget;
#[path = "sync_runtime.rs"]
mod runtime;

#[tokio::test]
#[ignore = "Requires an isolated PostgreSQL 17 TEST_POSTGRES_URL"]
async fn trading212_sync_identity_schema() {
    let url = std::env::var("TEST_POSTGRES_URL").expect("isolated TEST_POSTGRES_URL required");
    let (mut db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(connection);
    let schema = format!("t212_identity_{}", uuid::Uuid::new_v4().simple());
    db.batch_execute(&format!("CREATE SCHEMA {schema}; SET search_path TO {schema}")).await.unwrap();
    refinery::embed_migrations!("migrations");
    migrations::runner().set_target(refinery::Target::Version(35)).run_async(&mut db).await.unwrap();
    let owner = UserId::new();
    let other = UserId::new();
    for (id, username) in [(&owner, "sync-owner"), (&other, "sync-other")] {
        db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status)
            VALUES($1,$2,'fixture-password','fixture-totp','member','active')", &[&id.as_str(), &username]).await.unwrap();
        let tx = db.transaction().await.unwrap();
        auth::initialize_private_defaults(id, &tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    db.execute("UPDATE trading_connection SET environment='live',credentials='fixture',account=$2 WHERE user_id=$1",
        &[&owner.as_str(), &json!({"id":"broker-account", "currency":"EUR"})]).await.unwrap();
    let demo_owner = UserId::new();
    db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status)
        VALUES($1,'sync-demo','fixture-password','fixture-totp','member','active')", &[&demo_owner.as_str()]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    auth::initialize_private_defaults(&demo_owner, &tx).await.unwrap();
    tx.commit().await.unwrap();
    db.execute("UPDATE trading_connection SET environment='demo',credentials='fixture',account=$2 WHERE user_id=$1",
        &[&demo_owner.as_str(), &json!({"id":"demo-broker", "currency":"USD"})]).await.unwrap();
    migrations::runner().run_async(&mut db).await.unwrap();
    let account = db.query_one("SELECT * FROM t212_sync_account WHERE user_id=$1", &[&owner.as_str()]).await.unwrap();
    let account_id: String = account.get("id");
    assert!(!account.get::<_, bool>("schedule_enabled"));
    assert_eq!(account.get::<_, chrono::DateTime<Utc>>("backfill_cursor").timestamp(), 0);
    assert_eq!(db.query_one("SELECT count(*) FROM t212_sync_account WHERE user_id=$1", &[&other.as_str()]).await.unwrap().get::<_, i64>(0), 0);
    let backfilled = db.query_one("SELECT environment,broker_account_id,schedule_enabled FROM t212_sync_account WHERE user_id=$1", &[&demo_owner.as_str()]).await.unwrap();
    assert_eq!(backfilled.get::<_, String>("environment"), "demo");
    assert_eq!(backfilled.get::<_, String>("broker_account_id"), "demo-broker");
    assert!(!backfilled.get::<_, bool>("schedule_enabled"));
    db.execute("UPDATE t212_sync_account SET schedule_enabled=true,next_run_at='2030-01-01Z' WHERE user_id=$1 AND id=$2", &[&owner.as_str(), &account_id]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    trading212_import::lock_owner(&owner, &tx).await.unwrap();
    let reconnected = trading212_import::connection_changed(&owner, &tx, Some(("live", &json!({"id":"broker-account", "currency":"EUR"})))).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(reconnected, Some(account_id.clone()));
    assert!(db.query_one("SELECT schedule_enabled FROM t212_sync_account WHERE user_id=$1 AND id=$2", &[&owner.as_str(), &account_id]).await.unwrap().get::<_, bool>(0));
    assert!(db.execute("UPDATE t212_sync_account SET user_id=$1 WHERE user_id=$2 AND id=$3", &[&other.as_str(), &owner.as_str(), &account_id]).await.is_err());
    assert!(db.execute("INSERT INTO t212_import_batch(user_id,id,account_id,source,content_hash) VALUES($1,'foreign',$2,'statement_csv','digest')", &[&other.as_str(), &account_id]).await.is_err());
    let tx = db.transaction().await.unwrap();
    trading212_import::lock_owner(&owner, &tx).await.unwrap();
    let switched = trading212_import::connection_changed(&owner, &tx, Some(("live", &json!({"id":"second-account", "currency":"USD"})))).await.unwrap();
    tx.commit().await.unwrap();
    assert_ne!(switched, Some(account_id.clone()));
    assert_eq!(db.query_one("SELECT count(*) FROM t212_sync_account WHERE user_id=$1 AND schedule_enabled", &[&owner.as_str()]).await.unwrap().get::<_, i64>(0), 0);
    let tx = db.transaction().await.unwrap();
    trading212_import::lock_owner(&owner, &tx).await.unwrap();
    let demo_same_broker = trading212_import::connection_changed(&owner, &tx, Some(("demo", &json!({"id":"broker-account", "currency":"GBP"})))).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    assert_ne!(demo_same_broker, account_id);
    assert_eq!(db.query_one("SELECT environment FROM t212_sync_account WHERE user_id=$1 AND id=$2", &[&owner.as_str(), &demo_same_broker]).await.unwrap().get::<_, String>(0), "demo");
    db.execute("UPDATE t212_sync_account SET schedule_enabled=true,next_run_at='2030-01-01Z' WHERE user_id=$1 AND id=$2", &[&owner.as_str(), &demo_same_broker]).await.unwrap();
    let tx = db.transaction().await.unwrap();
    trading212_import::lock_owner(&owner, &tx).await.unwrap();
    let reconnected_demo = trading212_import::connection_changed(&owner, &tx, Some(("demo", &json!({"id":"broker-account", "currency":"GBP"})))).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(reconnected_demo.as_deref(), Some(demo_same_broker.as_str()));
    assert!(db.query_one("SELECT schedule_enabled FROM t212_sync_account WHERE user_id=$1 AND id=$2", &[&owner.as_str(), &demo_same_broker]).await.unwrap().get::<_, bool>(0));
    assert!(!db.query_one("SELECT schedule_enabled FROM t212_sync_account WHERE user_id=$1 AND environment='live' AND broker_account_id='broker-account'", &[&owner.as_str()]).await.unwrap().get::<_, bool>(0));
    assert!(db.execute("INSERT INTO t212_sync_account(user_id,id,environment,broker_account_id,currency) VALUES($1,'paper-account','paper','paper','EUR')", &[&owner.as_str()]).await.is_err());
    let before = db.query_one("SELECT count(*) FROM t212_sync_account WHERE user_id=$1", &[&owner.as_str()]).await.unwrap().get::<_, i64>(0);
    let tx = db.transaction().await.unwrap();
    trading212_import::lock_owner(&owner, &tx).await.unwrap();
    assert!(trading212_import::connection_changed(&owner, &tx, Some(("paper", &json!({"id":"paper", "currency":"EUR"})))).await.unwrap().is_none());
    tx.commit().await.unwrap();
    assert_eq!(db.query_one("SELECT count(*) FROM t212_sync_account WHERE user_id=$1", &[&owner.as_str()]).await.unwrap().get::<_, i64>(0), before);
    db.batch_execute(&format!("SET search_path TO public; DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
