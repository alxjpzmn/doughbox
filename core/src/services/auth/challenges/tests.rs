use std::{ffi::OsString, sync::Arc};

use argon2::{password_hash::SaltString, Algorithm, Argon2, Params, PasswordHasher, Version};
use chrono::{Duration, TimeZone};
use tokio_postgres::Client;

use super::*;
use crate::{
    database::models::auth::{CredentialKind, PendingTotp, Role, UserId},
    services::auth::{
        session_store::{create_anonymous, load, set_pending_locked},
        tests::{second_connection, session_created_at, test_database_at},
        AuthConfig,
    },
};

const PASSWORD: &str = "a complete test password";
const SECRET: &[u8] = b"12345678901234567890";
const CODE_A: &str = "01234567-89ABCDEF-01234567-89ABCDEF";
const CODE_B: &str = "11234567-89ABCDEF-01234567-89ABCDEF";
const CODE_C: &str = "21234567-89ABCDEF-01234567-89ABCDEF";

// As with the existing service fixtures, run ignored DB cases single-threaded.
// Fresh service connections point only at this dedicated, migrated schema.
struct Database {
    client: Client,
    schema: String,
    previous_url: Option<OsString>,
    service: Arc<AuthService>,
}

impl Database {
    async fn new() -> Self {
        let (client, schema) = test_database_at(refinery::Target::Latest).await;
        let test_url = std::env::var("TEST_POSTGRES_URL").expect("TEST_POSTGRES_URL is mandatory");
        let mut scoped = reqwest::Url::parse(&test_url).unwrap();
        let options = scoped
            .query_pairs()
            .find(|(key, _)| key == "options")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        let pairs: Vec<_> = scoped
            .query_pairs()
            .filter(|(key, _)| key != "options")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        scoped.set_query(None);
        scoped
            .query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("options", &format!("{options} -csearch_path={schema}"));
        let query = scoped.query().unwrap().replace('+', "%20");
        scoped.set_query(Some(&query));
        let previous_url = std::env::var_os("POSTGRES_URL");
        std::env::set_var("POSTGRES_URL", scoped.as_str());
        let service = new_service();
        Self {
            client,
            schema,
            previous_url,
            service,
        }
    }

    async fn user(&self, username: &str, status: AccountStatus) -> AuthUser {
        let id = UserId::new();
        let hash = self.service.passwords.hash(PASSWORD.into()).await.unwrap();
        let secret = self.service.encrypt_totp(&id, false, SECRET).unwrap();
        let factor = (status != AccountStatus::Pending).then_some(secret);
        self.client.execute(
            "INSERT INTO auth_users(id,username,password_hash,role,status,totp_secret_encrypted) VALUES($1,$2,$3,'member',$4,$5)",
            &[&id.as_str(), &username, &hash, &status.as_str(), &factor],
        ).await.unwrap();
        auth::by_id(&id, &self.client).await.unwrap().unwrap()
    }

    async fn issue(&mut self, grant: SessionGrant) -> IssuedSession {
        let tx = self.client.transaction().await.unwrap();
        let now = Utc::now();
        let anonymous = create_anonymous(&tx, now).await.unwrap();
        let issued = issue_successor_locked(&tx, &anonymous.session, grant, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn anonymous(&mut self) -> IssuedSession {
        let tx = self.client.transaction().await.unwrap();
        let issued = create_anonymous(&tx, Utc::now()).await.unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn recovery(&self, user: &AuthUser, code: &str) {
        let hash = recovery_hash(&user.id, code).unwrap();
        self.client
            .execute(
                "INSERT INTO auth_recovery_codes(user_id,code_hash) VALUES($1,$2)",
                &[&user.id.as_str(), &hash],
            )
            .await
            .unwrap();
    }

    async fn unused(&self, user: &AuthUser, code: &str) -> bool {
        let hash = recovery_hash(&user.id, code).unwrap();
        self.client
            .query_one(
                "SELECT used_at IS NULL FROM auth_recovery_codes WHERE user_id=$1 AND code_hash=$2",
                &[&user.id.as_str(), &hash],
            )
            .await
            .unwrap()
            .get(0)
    }

    async fn cleanup(&self) {
        self.client
            .batch_execute(&format!(
                "SET search_path TO public; DROP SCHEMA {} CASCADE",
                self.schema
            ))
            .await
            .unwrap();
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if let Some(url) = &self.previous_url {
            std::env::set_var("POSTGRES_URL", url);
        } else {
            std::env::remove_var("POSTGRES_URL");
        }
    }
}

fn new_service() -> Arc<AuthService> {
    AuthService::new(
        AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
    )
}

fn peer() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}

fn actor(user: &AuthUser) -> CurrentUser {
    user.current(CredentialKind::BrowserSession).unwrap()
}

fn full_grant(user: &AuthUser, proof: DateTime<Utc>, deadline: DateTime<Utc>) -> SessionGrant {
    SessionGrant::Authenticated {
        user_id: user.id.clone(),
        auth_version: user.auth_version,
        absolute_expires_at: deadline,
        reauthenticated_at: proof,
        pending_totp: None,
    }
}

fn error<T>(result: Result<T, AuthError>) -> AuthError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("unexpected authentication success"),
    }
}

fn old_hash() -> String {
    let salt = SaltString::encode_b64(b"fixed-test-salt!").unwrap();
    Argon2::new(
        Algorithm::Argon2i,
        Version::V0x10,
        Params::new(32, 1, 1, Some(16)).unwrap(),
    )
    .hash_password(PASSWORD.as_bytes(), &salt)
    .unwrap()
    .to_string()
}

#[test]
fn requests_reject_unknown_fields_and_preserve_code_strings() {
    assert!(serde_json::from_value::<LoginRequest>(
        serde_json::json!({"username":"alice","password":PASSWORD,"user_id":"other"})
    )
    .is_err());
    assert!(serde_json::from_value::<VerificationRequest>(
        serde_json::json!({"code":"081804","code_type":"totp","extra":true})
    )
    .is_err());
    assert!(serde_json::from_value::<ReauthRequest>(
        serde_json::json!({"password":PASSWORD,"code":"081804","code_type":"totp","extra":true})
    )
    .is_err());
    let request: VerificationRequest =
        serde_json::from_value(serde_json::json!({"code":"081804","code_type":"totp"})).unwrap();
    assert_eq!(request.code, "081804");
    assert!(request.code_type == CodeType::Totp);
    assert!(serde_json::from_value::<VerificationRequest>(
        serde_json::json!({"code":81804,"code_type":"totp"})
    )
    .is_err());
    assert!(serde_json::from_value::<VerificationRequest>(
        serde_json::json!({"code":"081804","code_type":"sms"})
    )
    .is_err());
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_password_login_is_restricted_and_denials_are_generic() {
    let mut db = Database::new().await;
    let active = db.user("alice", AccountStatus::Active).await;
    let pending = db.user("pending", AccountStatus::Pending).await;
    db.user("disabled", AccountStatus::Disabled).await;
    let anonymous = db.anonymous().await;
    for (username, password) in [
        ("alice", "wrong password"),
        ("unknown", PASSWORD),
        ("not a valid name", PASSWORD),
        ("disabled", PASSWORD),
    ] {
        let denied = error(
            db.service
                .login(&anonymous.session, username.into(), password.into(), peer())
                .await,
        );
        assert_eq!(denied.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(denied.code(), "InvalidCredentials");
        assert!(load(&db.client, &anonymous.raw_cookie, Utc::now())
            .await
            .unwrap()
            .is_some());
    }
    let challenge = db
        .service
        .login(
            &anonymous.session,
            "  ALICE ".into(),
            PASSWORD.into(),
            peer(),
        )
        .await
        .unwrap();
    assert!(challenge.session.stage == SessionStage::MfaRequired);
    assert_eq!(challenge.session.user_id.as_ref(), Some(&active.id));
    assert!(challenge.session.reauthenticated_at.is_none());
    assert!(load(&db.client, &anonymous.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert_ne!(challenge.session.csrf_token, anonymous.session.csrf_token);
    assert_eq!(
        challenge.session.absolute_expires_at
            - session_created_at(&db.client, &challenge.session).await,
        Duration::minutes(5)
    );
    let anonymous = db.anonymous().await;
    let enrollment = db
        .service
        .login(
            &anonymous.session,
            "pending".into(),
            PASSWORD.into(),
            peer(),
        )
        .await
        .unwrap();
    assert!(enrollment.session.stage == SessionStage::EnrollmentRequired);
    assert_eq!(enrollment.session.user_id.as_ref(), Some(&pending.id));
    assert!(enrollment.session.reauthenticated_at.is_none());
    assert_eq!(
        enrollment.session.absolute_expires_at
            - session_created_at(&db.client, &enrollment.session).await,
        Duration::minutes(10)
    );
    assert!(matches!(
        db.service
            .verify_mfa(&enrollment.session, CODE_A, CodeType::Recovery, peer())
            .await,
        Err(AuthError::Forbidden)
    ));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_login_phc_upgrade_requires_live_source_and_successful_commit() {
    let mut db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let old = old_hash();
    db.client
        .execute(
            "UPDATE auth_users SET password_hash=$2 WHERE id=$1",
            &[&user.id.as_str(), &old],
        )
        .await
        .unwrap();
    let stale = db.anonymous().await;
    db.service.logout(&stale.session).await.unwrap();
    assert!(matches!(
        db.service
            .login(&stale.session, "alice".into(), PASSWORD.into(), peer())
            .await,
        Err(AuthError::StateChanged)
    ));
    assert_eq!(
        auth::by_id(&user.id, &db.client)
            .await
            .unwrap()
            .unwrap()
            .password_hash
            .as_deref(),
        Some(old.as_str())
    );

    // A real deferred database error occurs at commit, after successor insertion.
    db.client
        .batch_execute(
            "CREATE FUNCTION fail_auth_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
           IF NEW.stage='mfa_required' THEN RAISE EXCEPTION 'test commit failure'; END IF;
           RETURN NEW;
         END $$;
         CREATE CONSTRAINT TRIGGER fail_auth_commit AFTER INSERT ON auth_sessions
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_auth_commit();",
        )
        .await
        .unwrap();
    let source = db.anonymous().await;
    assert!(matches!(
        db.service
            .login(&source.session, "alice".into(), PASSWORD.into(), peer())
            .await,
        Err(AuthError::Unavailable)
    ));
    assert!(load(&db.client, &source.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        auth::by_id(&user.id, &db.client)
            .await
            .unwrap()
            .unwrap()
            .password_hash
            .as_deref(),
        Some(old.as_str())
    );
    assert_eq!(
        db.client
            .query_one(
                "SELECT count(*) FROM auth_sessions WHERE stage='mfa_required'",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    db.client
        .batch_execute(
            "DROP TRIGGER fail_auth_commit ON auth_sessions; DROP FUNCTION fail_auth_commit()",
        )
        .await
        .unwrap();
    let challenge = db
        .service
        .login(&source.session, "alice".into(), PASSWORD.into(), peer())
        .await
        .unwrap();
    assert!(challenge.session.stage == SessionStage::MfaRequired);
    let upgraded = auth::by_id(&user.id, &db.client)
        .await
        .unwrap()
        .unwrap()
        .password_hash
        .unwrap();
    assert_ne!(upgraded, old);
    let verification = db
        .service
        .passwords
        .verify(PASSWORD.into(), Some(upgraded))
        .await
        .unwrap();
    assert!(verification.valid);
    assert!(verification.replacement_hash.is_none());
    db.cleanup().await;
}

async fn consume(
    service: &AuthService,
    client: &mut Client,
    owner: &UserId,
    code: &str,
    code_type: CodeType,
    now: DateTime<Utc>,
) -> bool {
    let tx = client.transaction().await.unwrap();
    let user = auth::lock_user(owner, &tx).await.unwrap();
    let accepted = service
        .verify_factor_locked(&tx, &user, code, code_type, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    accepted
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_totp_and_recovery_have_one_concurrent_winner_and_reject_replay() {
    let db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let other_user = db.user("bob", AccountStatus::Active).await;
    db.recovery(&user, CODE_A).await;
    let mut first = second_connection(&db.schema).await;
    let mut second = second_connection(&db.schema).await;
    let now = Utc.timestamp_opt(1_111_111_109, 0).unwrap();
    let (a, b) = tokio::join!(
        consume(
            &db.service,
            &mut first,
            &user.id,
            "081804",
            CodeType::Totp,
            now
        ),
        consume(
            &db.service,
            &mut second,
            &user.id,
            "081804",
            CodeType::Totp,
            now
        ),
    );
    assert_eq!(usize::from(a) + usize::from(b), 1);
    assert!(
        !consume(
            &db.service,
            &mut first,
            &user.id,
            "081804",
            CodeType::Totp,
            now
        )
        .await
    );
    assert_eq!(
        auth::by_id(&user.id, &db.client)
            .await
            .unwrap()
            .unwrap()
            .last_totp_step,
        Some(37_037_036)
    );
    assert!(
        !consume(
            &db.service,
            &mut first,
            &other_user.id,
            CODE_A,
            CodeType::Recovery,
            now
        )
        .await
    );
    let (a, b) = tokio::join!(
        consume(
            &db.service,
            &mut first,
            &user.id,
            CODE_A,
            CodeType::Recovery,
            now
        ),
        consume(
            &db.service,
            &mut second,
            &user.id,
            " 0123456789abcdef0123456789abcdef ",
            CodeType::Recovery,
            now
        ),
    );
    assert_eq!(usize::from(a) + usize::from(b), 1);
    assert!(!db.unused(&user, CODE_A).await);
    assert!(
        !consume(
            &db.service,
            &mut first,
            &user.id,
            CODE_A,
            CodeType::Recovery,
            now
        )
        .await
    );
    // Corruption is unavailable, not a successful factor or a normal bad code.
    db.client
        .execute(
            "UPDATE auth_users SET totp_secret_encrypted='corrupt' WHERE id=$1",
            &[&user.id.as_str()],
        )
        .await
        .unwrap();
    let tx = first.transaction().await.unwrap();
    let locked = auth::lock_user(&user.id, &tx).await.unwrap();
    assert!(matches!(
        db.service
            .verify_factor_locked(&tx, &locked, "081804", CodeType::Totp, now)
            .await,
        Err(AuthError::Unavailable)
    ));
    tx.rollback().await.unwrap();
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_mfa_failures_commit_revisions_and_delete_fifth_attempt() {
    let mut db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let challenge = db
        .issue(SessionGrant::MfaRequired {
            user_id: user.id.clone(),
            auth_version: user.auth_version,
        })
        .await;
    let mut source = challenge.session.clone();
    for failures in 1..=5 {
        assert!(matches!(
            db.service
                .verify_mfa(&source, "bad", CodeType::Totp, peer())
                .await,
            Err(AuthError::InvalidVerificationCode)
        ));
        let loaded = load(&db.client, &challenge.raw_cookie, Utc::now())
            .await
            .unwrap();
        if failures == 5 {
            assert!(loaded.is_none());
        } else {
            let loaded = loaded.unwrap();
            assert_eq!(loaded.failed_factor_attempts, failures);
            assert_eq!(loaded.revision, source.revision + 1);
            assert!(matches!(
                db.service
                    .verify_mfa(&source, "bad", CodeType::Totp, peer())
                    .await,
                Err(AuthError::StateChanged)
            ));
            source = loaded;
        }
    }
    db.recovery(&user, CODE_A).await;
    let challenge = db
        .issue(SessionGrant::MfaRequired {
            user_id: user.id.clone(),
            auth_version: user.auth_version,
        })
        .await;
    let full = db
        .service
        .verify_mfa(&challenge.session, CODE_A, CodeType::Recovery, peer())
        .await
        .unwrap();
    assert!(full.session.stage == SessionStage::Authenticated);
    let created_at = session_created_at(&db.client, &full.session).await;
    assert_eq!(
        full.session.absolute_expires_at - created_at,
        Duration::hours(12)
    );
    assert_eq!(full.session.reauthenticated_at, Some(created_at));
    assert!(full.session.pending_totp.is_none());
    assert_eq!(full.session.failed_factor_attempts, 0);
    assert!(load(&db.client, &challenge.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(!db.unused(&user, CODE_A).await);
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_reauthentication_preserves_deadline_and_only_unexpired_pending() {
    let mut db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let actor = actor(&user);
    for code in [CODE_A, CODE_B, CODE_C] {
        db.recovery(&user, code).await;
    }
    let now = Utc::now();
    let full = db
        .issue(full_grant(
            &user,
            now - Duration::minutes(6),
            now + Duration::hours(1),
        ))
        .await;
    let other = db.issue(SessionGrant::full(&user, now)).await;
    assert!(matches!(
        db.service
            .revoke_session(&actor, &full.session, &other.session.id_hash)
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    let pending = PendingTotp {
        id: uuid::Uuid::new_v4().to_string(),
        secret_encrypted: db.service.encrypt_totp(&user.id, true, SECRET).unwrap(),
        expires_at: now + Duration::minutes(10),
    };
    let tx = db.client.transaction().await.unwrap();
    auth::lock_user(&user.id, &tx).await.unwrap();
    let source = lock_source(&tx, &full.session, Utc::now()).await.unwrap();
    let source = set_pending_locked(&tx, &source, Some(&pending))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        db.service
            .reauthenticate(
                &actor,
                &source,
                "wrong password".into(),
                CODE_A,
                CodeType::Recovery,
                peer()
            )
            .await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(db.unused(&user, CODE_A).await);
    assert!(matches!(
        db.service
            .reauthenticate(
                &actor,
                &source,
                PASSWORD.into(),
                "bad",
                CodeType::Recovery,
                peer()
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    assert!(db.unused(&user, CODE_A).await);
    let before = Utc::now();
    let renewed = db
        .service
        .reauthenticate(
            &actor,
            &source,
            PASSWORD.into(),
            CODE_A,
            CodeType::Recovery,
            peer(),
        )
        .await
        .unwrap();
    assert_eq!(
        renewed.session.absolute_expires_at,
        source.absolute_expires_at
    );
    assert!(renewed.session.reauthenticated_at.unwrap() >= before);
    assert!(require_recent(&renewed.session, Utc::now()).is_ok());
    let retained = renewed.session.pending_totp.as_ref().unwrap();
    let previous = source.pending_totp.as_ref().unwrap();
    assert_eq!(retained.id, previous.id);
    assert_eq!(retained.secret_encrypted, previous.secret_encrypted);
    assert_eq!(retained.expires_at, previous.expires_at);
    assert_ne!(renewed.session.csrf_token, source.csrf_token);
    assert!(load(&db.client, &full.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(!db
        .service
        .revoke_session(&actor, &renewed.session, &other.session.id_hash)
        .await
        .unwrap());
    let tx = db.client.transaction().await.unwrap();
    auth::lock_user(&user.id, &tx).await.unwrap();
    let locked = lock_source(&tx, &renewed.session, Utc::now())
        .await
        .unwrap();
    let expired = PendingTotp {
        expires_at: Utc::now() - Duration::seconds(1),
        ..pending
    };
    let source = set_pending_locked(&tx, &locked, Some(&expired))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let renewed = db
        .service
        .reauthenticate(
            &actor,
            &source,
            PASSWORD.into(),
            CODE_B,
            CodeType::Recovery,
            peer(),
        )
        .await
        .unwrap();
    assert_eq!(
        renewed.session.absolute_expires_at,
        source.absolute_expires_at
    );
    assert!(renewed.session.pending_totp.is_none());
    assert!(matches!(
        db.service
            .reauthenticate(
                &actor,
                &renewed.session,
                PASSWORD.into(),
                CODE_B,
                CodeType::Recovery,
                peer()
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    assert!(db.unused(&user, CODE_C).await);
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_sessions_persist_list_own_full_and_revoke_without_resurrection() {
    let mut db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let other_user = db.user("bob", AccountStatus::Active).await;
    let actor = actor(&user);
    let now = Utc::now();
    let current = db.issue(SessionGrant::full(&user, now)).await;
    let other = db.issue(SessionGrant::full(&user, now)).await;
    let restricted = db
        .issue(SessionGrant::MfaRequired {
            user_id: user.id.clone(),
            auth_version: user.auth_version,
        })
        .await;
    let foreign = db.issue(SessionGrant::full(&other_user, now)).await;
    let expired = db
        .issue(full_grant(
            &user,
            now - Duration::hours(1),
            now + Duration::hours(1),
        ))
        .await;
    db.client
        .execute(
            "UPDATE auth_sessions SET expires_at=$2 WHERE id_hash=$1",
            &[&expired.session.id_hash, &(now - Duration::seconds(1))],
        )
        .await
        .unwrap();
    let replica = new_service();
    let connection = second_connection(&db.schema).await;
    let restored = load(&connection, &current.raw_cookie, Utc::now())
        .await
        .unwrap()
        .unwrap();
    let listed = replica.sessions(&actor, &restored).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed.iter().filter(|session| session.current).count(), 1);
    assert!(listed
        .iter()
        .any(|session| session.id == other.session.id_hash));
    assert!(!listed
        .iter()
        .any(|session| session.id == restricted.session.id_hash));
    assert!(matches!(
        replica
            .revoke_session(&actor, &restored, &foreign.session.id_hash)
            .await,
        Err(AuthError::NotFound)
    ));
    assert!(matches!(
        replica
            .revoke_session(&actor, &restored, "missing-id")
            .await,
        Err(AuthError::NotFound)
    ));
    assert!(!replica
        .revoke_session(&actor, &restored, &other.session.id_hash)
        .await
        .unwrap());
    assert!(load(&connection, &other.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        replica.logout(&other.session).await,
        Err(AuthError::StateChanged)
    ));
    db.client
        .execute(
            "UPDATE auth_sessions SET reauthenticated_at=$2 WHERE id_hash=$1",
            &[
                &current.session.id_hash,
                &(Utc::now() - Duration::minutes(6)),
            ],
        )
        .await
        .unwrap();
    // Logging out this session itself never needs a fresh password/factor proof.
    assert!(replica
        .revoke_session(&actor, &restored, &current.session.id_hash)
        .await
        .unwrap());
    assert!(load(&connection, &current.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        replica.sessions(&actor, &restored).await,
        Err(AuthError::StateChanged)
    ));
    assert!(load(&connection, &foreign.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_revocation_winning_source_race_cannot_mint_successor_or_consume_factor() {
    let mut db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let actor = actor(&user);
    db.recovery(&user, CODE_A).await;
    db.recovery(&user, CODE_B).await;
    let full = db.issue(SessionGrant::full(&user, Utc::now())).await;
    let service = db.service.clone();
    let tx = db.client.transaction().await.unwrap();
    auth::lock_user(&user.id, &tx).await.unwrap();
    let revoke = async {
        let source = lock_source(&tx, &full.session, Utc::now()).await.unwrap();
        delete_locked(&tx, &source).await.unwrap();
        tx.commit().await.unwrap();
    };
    let (denied, ()) = tokio::join!(
        service.reauthenticate(
            &actor,
            &full.session,
            PASSWORD.into(),
            CODE_A,
            CodeType::Recovery,
            peer()
        ),
        revoke,
    );
    assert!(matches!(denied, Err(AuthError::StateChanged)));
    assert!(db.unused(&user, CODE_A).await);
    assert!(load(&db.client, &full.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    let challenge = db
        .issue(SessionGrant::MfaRequired {
            user_id: user.id.clone(),
            auth_version: user.auth_version,
        })
        .await;
    service.logout(&challenge.session).await.unwrap();
    assert!(matches!(
        service
            .verify_mfa(&challenge.session, CODE_B, CodeType::Recovery, peer())
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(db.unused(&user, CODE_B).await);
    assert_eq!(
        db.client
            .query_one(
                "SELECT count(*) FROM auth_sessions WHERE user_id=$1",
                &[&user.id.as_str()]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_captured_actor_role_version_and_source_are_not_refreshed() {
    let mut db = Database::new().await;
    let user = db.user("alice", AccountStatus::Active).await;
    let actor = actor(&user);
    let full = db.issue(SessionGrant::full(&user, Utc::now())).await;
    let mut token_actor = actor.clone();
    token_actor.credential = CredentialKind::PersonalReadToken;
    assert!(matches!(
        db.service.sessions(&token_actor, &full.session).await,
        Err(AuthError::Forbidden)
    ));
    db.client
        .execute(
            "UPDATE auth_users SET role=$2 WHERE id=$1",
            &[&user.id.as_str(), &Role::Admin.as_str()],
        )
        .await
        .unwrap();
    assert!(matches!(
        db.service.sessions(&actor, &full.session).await,
        Err(AuthError::StateChanged)
    ));
    db.client
        .execute(
            "UPDATE auth_users SET role='member',auth_version=auth_version+1 WHERE id=$1",
            &[&user.id.as_str()],
        )
        .await
        .unwrap();
    assert!(matches!(
        db.service
            .revoke_session(&actor, &full.session, &full.session.id_hash)
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(load(&db.client, &full.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    db.cleanup().await;
}
