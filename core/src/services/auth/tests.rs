use chrono::{DateTime, Duration, Utc};
use tokio_postgres::{Client, NoTls};

use super::{session_store::*, AuthConfig, AuthError, AuthService};
use crate::database::{models::auth::*, queries::auth};

pub(crate) async fn test_database_at(target: refinery::Target) -> (Client, String) {
    let url = std::env::var("TEST_POSTGRES_URL")
        .expect("Set TEST_POSTGRES_URL to a dedicated PostgreSQL test database");
    let (mut db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    let schema = format!(
        "auth_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
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

pub(crate) async fn second_connection(schema: &str) -> Client {
    let url = std::env::var("TEST_POSTGRES_URL").expect("TEST_POSTGRES_URL is mandatory");
    let (db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    db.batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    db
}

pub(crate) async fn session_created_at(db: &Client, session: &AuthSession) -> DateTime<Utc> {
    db.query_one(
        "SELECT created_at FROM auth_sessions WHERE id_hash=$1",
        &[&session.id_hash],
    )
    .await
    .unwrap()
    .get(0)
}

pub(crate) async fn cleanup(db: Client, schema: &str) {
    db.batch_execute(&format!(
        "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
    ))
    .await
    .unwrap();
}

async fn activate_reserved(db: &Client) -> AuthUser {
    let id = auth::legacy_owner(db).await.unwrap();
    db.execute("UPDATE auth_users SET username='alice',password_hash='test-phc',totp_secret_encrypted='test-envelope',status='active' WHERE id=$1", &[&id.as_str()]).await.unwrap();
    auth::by_id(&id, db).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
async fn auth_database_sessions_persist_rotate_and_lose_revocation_races() {
    let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
    let user = activate_reserved(&db).await;
    let now = Utc::now();
    let tx = db.transaction().await.unwrap();
    let anonymous = create_anonymous(&tx, now).await.unwrap();
    tx.commit().await.unwrap();
    let mut other = second_connection(&schema).await;
    let loaded = load(&other, &anonymous.raw_cookie, now)
        .await
        .unwrap()
        .unwrap();
    assert!(loaded.stage == SessionStage::Anonymous);
    let tx = db.transaction().await.unwrap();
    let source = lock_source(&tx, &loaded, now).await.unwrap();
    let challenge = issue_successor_locked(
        &tx,
        &source,
        SessionGrant::MfaRequired {
            user_id: user.id.clone(),
            auth_version: user.auth_version,
        },
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(load(&other, &anonymous.raw_cookie, now)
        .await
        .unwrap()
        .is_none());
    assert!(
        load(&other, &challenge.raw_cookie, now)
            .await
            .unwrap()
            .unwrap()
            .stage
            == SessionStage::MfaRequired
    );
    let tx = db.transaction().await.unwrap();
    let source = lock_source(&tx, &challenge.session, now).await.unwrap();
    let full = issue_successor_locked(&tx, &source, SessionGrant::full(&user, now), now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stale = load(&other, &full.raw_cookie, now + Duration::minutes(1))
        .await
        .unwrap()
        .unwrap();
    let tx = other.transaction().await.unwrap();
    let source = lock_source(&tx, &stale, now + Duration::minutes(1))
        .await
        .unwrap();
    delete_locked(&tx, &source).await.unwrap();
    tx.commit().await.unwrap();
    let tx = db.transaction().await.unwrap();
    assert!(matches!(
        lock_source(&tx, &stale, now + Duration::minutes(1)).await,
        Err(AuthError::StateChanged)
    ));
    tx.rollback().await.unwrap();
    assert_eq!(
        db.query_one(
            "SELECT count(*) FROM auth_sessions WHERE user_id=$1",
            &[&user.id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    cleanup(db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
async fn auth_database_pending_revisions_and_idle_absolute_expiry() {
    let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
    let user = activate_reserved(&db).await;
    let now = Utc::now();
    let tx = db.transaction().await.unwrap();
    let full = super::session_store::create_anonymous(&tx, now)
        .await
        .unwrap();
    let full = issue_successor_locked(&tx, &full.session, SessionGrant::full(&user, now), now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let tx = db.transaction().await.unwrap();
    let source = lock_source(&tx, &full.session, now).await.unwrap();
    let pending = PendingTotp {
        id: uuid::Uuid::new_v4().to_string(),
        secret_encrypted: "ciphertext".into(),
        expires_at: now + Duration::minutes(10),
    };
    let revised = set_pending_locked(&tx, &source, Some(&pending))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let tx = db.transaction().await.unwrap();
    assert!(matches!(
        lock_source(&tx, &full.session, now).await,
        Err(AuthError::StateChanged)
    ));
    tx.rollback().await.unwrap();
    let touched = load(&db, &full.raw_cookie, now + Duration::minutes(20))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(touched.revision, revised.revision);
    assert_eq!(touched.pending_totp.unwrap().id, pending.id);
    assert!(load(&db, &full.raw_cookie, now + Duration::minutes(51))
        .await
        .unwrap()
        .is_none());
    assert!(load(&db, &full.raw_cookie, now + Duration::hours(12))
        .await
        .unwrap()
        .is_none());
    cleanup(db, &schema).await;
}

#[test]
fn cookie_policy_and_encrypted_factor_are_bound_to_configuration_and_owner() {
    let key = "17".repeat(32);
    assert!(AuthConfig::parse("http://localhost:8084", &key, false).is_err());
    assert!(AuthConfig::parse("https://localhost/portfolio", &key, false).is_err());
    assert!(AuthConfig::parse("https://localhost", "not-a-key", false).is_err());
    for (origin, secure) in [
        ("https://EXAMPLE.test:443/", true),
        ("http://localhost:8084", false),
    ] {
        let service = AuthService::new(AuthConfig::parse(origin, &key, !secure).unwrap());
        let cookie = service.session_cookie(
            "test-only-cookie".into(),
            Utc::now() + Duration::minutes(30),
        );
        assert_eq!(cookie.secure(), Some(secure));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(
            cookie.same_site(),
            Some(tower_cookies::cookie::SameSite::Lax)
        );
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.domain(), None);
        let a = UserId::new();
        let b = UserId::new();
        let pending = service
            .encrypt_totp(&a, true, b"twenty-byte-secret!!")
            .unwrap();
        assert_eq!(
            service.decrypt_totp(&a, true, &pending).unwrap(),
            b"twenty-byte-secret!!"
        );
        assert!(matches!(
            service.decrypt_totp(&a, false, &pending),
            Err(AuthError::Unavailable)
        ));
        assert!(matches!(
            service.decrypt_totp(&b, true, &pending),
            Err(AuthError::Unavailable)
        ));
    }
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
async fn auth_database_v33_preserves_archived_ledgers_and_private_controls() {
    let (mut db, schema) = test_database_at(refinery::Target::Version(32)).await;
    db.batch_execute(
        "INSERT INTO asset(id,name,asset_class,currency,unit_label,current_interest_rate_percent,created_at,updated_at)
         VALUES('cash','Historical savings','CashAccount','EUR','EUR',2,'2024-01-01Z','2024-01-01Z');
         INSERT INTO asset_balance_snapshot(id,asset_id,date,kind,balance,balance_eur)
         VALUES('opening','cash','2024-01-01Z','Opening',100,100);
         INSERT INTO interest(id,asset_id,date,amount,amount_eur,principal,currency)
         VALUES('legacy-interest','cash','2024-02-01Z',5,5,'Cash','EUR');
         UPDATE asset SET archived_at='2025-01-01Z' WHERE id='cash';
         UPDATE telegram_report_schedule SET enabled=TRUE,send_time_utc='12:34',next_report_at='2027-01-01Z';
         UPDATE trading_connection SET credentials='legacy-ciphertext',environment='demo',
                agent_token_hash='old-agent',generation='legacy-generation';"
    ).await.unwrap();
    let before: serde_json::Value = db
        .query_one("SELECT to_jsonb(a) FROM asset a", &[])
        .await
        .unwrap()
        .get(0);
    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    let after: serde_json::Value = db
        .query_one("SELECT to_jsonb(a)-'user_id' FROM asset a", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
    let owner = auth::legacy_owner(&db).await.unwrap();
    let row = db
        .query_one(
            "SELECT user_id,asset_id,amount FROM interest WHERE id='legacy-interest'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), owner.as_str());
    assert_eq!(row.get::<_, &str>(1), "cash");
    assert_eq!(
        row.get::<_, rust_decimal::Decimal>(2),
        rust_decimal::Decimal::from(5)
    );
    let row = db
        .query_one(
            "SELECT enabled,send_time_utc FROM telegram_report_schedule WHERE user_id=$1",
            &[&owner.as_str()],
        )
        .await
        .unwrap();
    assert!(row.get::<_, bool>(0));
    assert_eq!(row.get::<_, &str>(1), "12:34");
    let row=db.query_one("SELECT credentials,generation,agent_token_hash,agent_auth_version FROM trading_connection WHERE user_id=$1", &[&owner.as_str()]).await.unwrap();
    assert_eq!(row.get::<_, &str>(0), "legacy-ciphertext");
    assert_eq!(row.get::<_, &str>(1), "legacy-generation");
    assert!(row.get::<_, Option<String>>(2).is_none());
    assert!(row.get::<_, Option<i64>>(3).is_none());
    let disabled:i64=db.query_one("SELECT count(*) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND NOT t.tgisinternal AND t.tgenabled='D'", &[&schema]).await.unwrap().get(0);
    assert_eq!(disabled, 0);
    assert!(auth::setup_required(&db).await.unwrap());
    cleanup(db, &schema).await;
}
