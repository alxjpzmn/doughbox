use std::{ffi::OsString, net::IpAddr, sync::Arc};

use chrono::{Duration, Utc};
use tokio_postgres::Client;
use totp_rs::{Algorithm, TOTP};

use crate::{
    database::models::auth::{AccountStatus, CredentialKind},
    services::auth::{
        challenges::CodeType,
        session_store::{create_anonymous, load, session_from_row},
        tests::{cleanup, test_database_at},
        AuthConfig,
    },
};

use super::*;

const PASSWORD: &str = "initial fixture password";
const OLD_SECRET: &[u8] = b"12345678901234567890";

// Services open ordinary connections. Scope every one to this freshly migrated
// TEST_POSTGRES_URL schema, and restore the caller's configuration even on panic.
struct ScopedDatabaseUrl(Option<OsString>);

impl ScopedDatabaseUrl {
    fn new(schema: &str) -> Self {
        let mut url = reqwest::Url::parse(
            &std::env::var("TEST_POSTGRES_URL").expect("TEST_POSTGRES_URL is mandatory"),
        )
        .unwrap();
        url.query_pairs_mut()
            .append_pair("options", &format!("-c search_path={schema}"));
        let query = url.query().unwrap().replace('+', "%20");
        url.set_query(Some(&query));
        let previous = std::env::var_os("POSTGRES_URL");
        std::env::set_var("POSTGRES_URL", url.as_str());
        Self(previous)
    }
}

impl Drop for ScopedDatabaseUrl {
    fn drop(&mut self) {
        match &self.0 {
            Some(previous) => std::env::set_var("POSTGRES_URL", previous),
            None => std::env::remove_var("POSTGRES_URL"),
        }
    }
}

struct Fixture {
    db: Client,
    schema: String,
    service: Arc<AuthService>,
    user_id: UserId,
    _url: ScopedDatabaseUrl,
}

impl Fixture {
    async fn new(pending: bool) -> Self {
        let (db, schema) = test_database_at(refinery::Target::Latest).await;
        let url = ScopedDatabaseUrl::new(&schema);
        let service = AuthService::new(
            AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
        );
        let user_id = auth::legacy_owner(&db).await.unwrap();
        let hash = service.passwords.hash(PASSWORD.to_owned()).await.unwrap();
        let secret = if pending {
            None
        } else {
            Some(service.encrypt_totp(&user_id, false, OLD_SECRET).unwrap())
        };
        let status = if pending { "pending" } else { "active" };
        db.execute(
            "UPDATE auth_users SET username='alice',password_hash=$2,totp_secret_encrypted=$3,status=$4 WHERE id=$1",
            &[&user_id.as_str(), &hash, &secret, &status],
        ).await.unwrap();
        Self {
            db,
            schema,
            service,
            user_id,
            _url: url,
        }
    }

    async fn user(&self) -> AuthUser {
        auth::by_id(&self.user_id, &self.db).await.unwrap().unwrap()
    }

    async fn actor(&self) -> CurrentUser {
        self.user()
            .await
            .current(CredentialKind::BrowserSession)
            .unwrap()
    }

    async fn issue(&mut self, stage: SessionStage) -> IssuedSession {
        let user = self.user().await;
        let now = Utc::now();
        let tx = self.db.transaction().await.unwrap();
        let anonymous = create_anonymous(&tx, now).await.unwrap();
        let grant = match stage {
            SessionStage::Anonymous => {
                tx.commit().await.unwrap();
                return anonymous;
            }
            SessionStage::MfaRequired => SessionGrant::MfaRequired {
                user_id: user.id,
                auth_version: user.auth_version,
            },
            SessionStage::EnrollmentRequired => SessionGrant::EnrollmentRequired {
                user_id: user.id,
                auth_version: user.auth_version,
            },
            SessionStage::Authenticated => SessionGrant::Authenticated {
                user_id: user.id,
                auth_version: user.auth_version,
                absolute_expires_at: now + Duration::hours(6),
                reauthenticated_at: now - Duration::minutes(2),
                pending_totp: None,
            },
        };
        let issued = issue_successor_locked(&tx, &anonymous.session, grant, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn stored(&self, source: &AuthSession) -> AuthSession {
        session_from_row(
            self.db
                .query_one(
                    "SELECT * FROM auth_sessions WHERE id_hash=$1",
                    &[&source.id_hash],
                )
                .await
                .unwrap(),
        )
        .unwrap()
    }

    async fn cleanup(self) {
        cleanup(self.db, &self.schema).await;
    }
}

fn peer() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}

fn enrollment_code(response: &TotpStartResponse) -> String {
    TOTP::from_url(&response.otpauth_uri)
        .unwrap()
        .generate(Utc::now().timestamp() as u64)
}

fn old_code() -> String {
    TOTP::new(
        Algorithm::SHA1,
        6,
        0,
        30,
        OLD_SECRET.to_vec(),
        None,
        String::new(),
    )
    .unwrap()
    .generate(Utc::now().timestamp() as u64)
}

#[test]
fn credential_requests_reject_unknown_fields() {
    assert!(serde_json::from_value::<EmptyRequest>(serde_json::json!({})).is_ok());
    assert!(serde_json::from_value::<EmptyRequest>(serde_json::json!({"code":"123456"})).is_err());
    assert!(
        serde_json::from_value::<TotpConfirmRequest>(serde_json::json!({
            "enrollment_id":"test", "code":"012345", "user_id":"other",
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<PendingTotpRequest>(serde_json::json!({
            "enrollment_id":"test", "active":false,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<PasswordChangeRequest>(serde_json::json!({
            "new_password":"a valid new password", "username":"other",
        }))
        .is_err()
    );
    let request = serde_json::from_value::<TotpConfirmRequest>(serde_json::json!({
        "enrollment_id":"test", "code":"012345",
    }))
    .unwrap();
    assert_eq!(request.code, "012345");
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_pending_enrollment_is_session_bound_and_expired_failures_commit() {
    let mut f = Fixture::new(true).await;
    let first = f.issue(SessionStage::EnrollmentRequired).await;
    let second = f.issue(SessionStage::EnrollmentRequired).await;
    let (first, a) = f
        .service
        .totp_start(&first.session, None, peer())
        .await
        .unwrap();
    let (second, b) = f
        .service
        .totp_start(&second.session, None, peer())
        .await
        .unwrap();
    assert_ne!(a.enrollment_id, b.enrollment_id);
    assert!(matches!(
        f.service
            .totp_confirm(
                &second,
                None,
                &a.enrollment_id,
                &enrollment_code(&a),
                peer(),
            )
            .await,
        Err(AuthError::StateChanged)
    ));
    let user = f.user().await;
    assert!(user.status == AccountStatus::Pending);
    assert!(user.totp_secret_encrypted.is_none());

    f.db.execute(
        "UPDATE auth_sessions SET pending_totp_expires_at=$2,revision=revision+1 WHERE id_hash=$1",
        &[&first.id_hash, &(Utc::now() - Duration::seconds(1))],
    )
    .await
    .unwrap();
    let mut expired = f.stored(&first).await;
    for attempt in 1..=5 {
        assert!(matches!(
            f.service
                .totp_confirm(
                    &expired,
                    None,
                    &a.enrollment_id,
                    &enrollment_code(&a),
                    peer(),
                )
                .await,
            Err(AuthError::InvalidVerificationCode)
        ));
        if attempt < 5 {
            let previous_revision = expired.revision;
            expired = f.stored(&expired).await;
            assert_eq!(expired.failed_factor_attempts, attempt);
            assert_eq!(expired.revision, previous_revision + 1);
        }
    }
    assert!(f
        .db
        .query_opt(
            "SELECT 1 FROM auth_sessions WHERE id_hash=$1",
            &[&first.id_hash]
        )
        .await
        .unwrap()
        .is_none());
    assert!(f.user().await.totp_secret_encrypted.is_none());
    let cancelled = f
        .service
        .totp_cancel(&second, None, &b.enrollment_id)
        .await
        .unwrap();
    assert!(cancelled.stage == SessionStage::EnrollmentRequired);
    assert!(cancelled.pending_totp.is_none());
    assert!(matches!(
        f.service
            .totp_confirm(
                &second,
                None,
                &b.enrollment_id,
                &enrollment_code(&b),
                peer(),
            )
            .await,
        Err(AuthError::StateChanged)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_pending_start_cancel_confirm_races_have_one_revision_winner() {
    let mut f = Fixture::new(false).await;
    let original = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let (source, response) = f
        .service
        .totp_start(&original.session, Some(&actor), peer())
        .await
        .unwrap();
    let code = enrollment_code(&response);
    let (start, confirm) = tokio::join!(
        f.service.totp_start(&source, Some(&actor), peer()),
        f.service.totp_confirm(
            &source,
            Some(&actor),
            &response.enrollment_id,
            &code,
            peer()
        ),
    );
    assert_ne!(start.is_ok(), confirm.is_ok());
    let current = match (start, confirm) {
        (Ok((next, _)), Err(AuthError::StateChanged)) => next,
        (Err(AuthError::StateChanged), Ok((next, _))) => next.session,
        _ => panic!("only one pending-secret revision may win"),
    };
    let actor = f.actor().await;
    let (source, response) = f
        .service
        .totp_start(&current, Some(&actor), peer())
        .await
        .unwrap();
    let previous_secret = f.user().await.totp_secret_encrypted;
    let previous_version = actor.auth_version;
    let code = enrollment_code(&response);
    let (cancel, confirm) = tokio::join!(
        f.service
            .totp_cancel(&source, Some(&actor), &response.enrollment_id),
        f.service.totp_confirm(
            &source,
            Some(&actor),
            &response.enrollment_id,
            &code,
            peer()
        ),
    );
    assert_ne!(cancel.is_ok(), confirm.is_ok());
    let current = match (cancel, confirm) {
        (Ok(next), Err(AuthError::StateChanged)) => {
            assert!(next.pending_totp.is_none());
            assert_eq!(f.user().await.totp_secret_encrypted, previous_secret);
            assert_eq!(f.user().await.auth_version, previous_version);
            next
        }
        (Err(AuthError::StateChanged), Ok((next, _))) => {
            assert_eq!(f.user().await.auth_version, previous_version + 1);
            next.session
        }
        _ => panic!("cancel and confirm cannot both mutate a captured source"),
    };
    let actor = f.actor().await;
    let (a, b) = tokio::join!(
        f.service.totp_start(&current, Some(&actor), peer()),
        f.service.totp_start(&current, Some(&actor), peer()),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(AuthError::StateChanged)) | (Err(AuthError::StateChanged), Ok(_))
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_failed_replacement_preserves_old_factor_and_success_records_replay_step() {
    let mut f = Fixture::new(false).await;
    let original = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let before = f.user().await;
    let (source, response) = f
        .service
        .totp_start(&original.session, Some(&actor), peer())
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .totp_confirm(
                &source,
                Some(&actor),
                &response.enrollment_id,
                "not-an-otp",
                peer(),
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    assert_eq!(
        f.user().await.totp_secret_encrypted,
        before.totp_secret_encrypted
    );
    assert_eq!(f.user().await.auth_version, before.auth_version);
    assert_eq!(f.stored(&source).await.failed_factor_attempts, 0);
    let challenge = f.issue(SessionStage::MfaRequired).await;
    let old_factor_login = f
        .service
        .verify_mfa(&challenge.session, &old_code(), CodeType::Totp, peer())
        .await
        .unwrap();
    assert!(old_factor_login.session.stage == SessionStage::Authenticated);

    let code = enrollment_code(&response);
    let (replacement, codes) = f
        .service
        .totp_confirm(
            &source,
            Some(&actor),
            &response.enrollment_id,
            &code,
            peer(),
        )
        .await
        .unwrap();
    let user = f.user().await;
    assert!(user.status == AccountStatus::Active);
    assert_eq!(user.auth_version, before.auth_version + 1);
    let active = f
        .service
        .decrypt_totp(
            &user.id,
            false,
            user.totp_secret_encrypted.as_deref().unwrap(),
        )
        .unwrap();
    let accepted =
        totp::verify_totp_step(&active, &code, Utc::now().timestamp() as u64, None).unwrap();
    assert_eq!(user.last_totp_step, Some(accepted));
    assert_eq!(
        replacement.session.absolute_expires_at,
        original.session.absolute_expires_at
    );
    assert!(replacement.session.reauthenticated_at > original.session.reauthenticated_at);
    assert!(replacement.session.pending_totp.is_none());
    assert_ne!(replacement.session.id_hash, source.id_hash);
    assert_ne!(replacement.session.csrf_token, source.csrf_token);
    assert_eq!(codes.len(), 10);
    assert!(load(&f.db, &original.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(load(&f.db, &old_factor_login.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());

    let challenge = f.issue(SessionStage::MfaRequired).await;
    assert!(matches!(
        f.service
            .verify_mfa(&challenge.session, &code, CodeType::Totp, peer(),)
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    // Even if the confirmation response is lost, password plus the now-active
    // authenticator (its next accepted step) can establish a new full session.
    let anonymous = f.issue(SessionStage::Anonymous).await;
    let login = f
        .service
        .login(&anonymous.session, "alice".into(), PASSWORD.into(), peer())
        .await
        .unwrap();
    assert!(login.session.stage == SessionStage::MfaRequired);
    let next_code = TOTP::from_url(&response.otpauth_uri)
        .unwrap()
        .generate((accepted as u64 + 1) * 30);
    let recovered = f
        .service
        .verify_mfa(&login.session, &next_code, CodeType::Totp, peer())
        .await
        .unwrap();
    assert!(recovered.session.stage == SessionStage::Authenticated);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_recovery_regeneration_invalidates_old_set_and_concurrent_reuse_loses() {
    let mut f = Fixture::new(false).await;
    let original = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let (first, old_codes) = f
        .service
        .regenerate_recovery_codes(&actor, &original.session)
        .await
        .unwrap();
    let (pending, _) = f
        .service
        .totp_start(&first.session, Some(&actor), peer())
        .await
        .unwrap();
    let (second, codes) = f
        .service
        .regenerate_recovery_codes(&actor, &pending)
        .await
        .unwrap();
    assert_eq!(second.session.auth_version, original.session.auth_version);
    assert_eq!(
        second.session.absolute_expires_at,
        original.session.absolute_expires_at
    );
    assert_eq!(
        second.session.reauthenticated_at,
        original.session.reauthenticated_at
    );
    assert!(second.session.pending_totp.is_none());
    assert_ne!(second.session.csrf_token, first.session.csrf_token);
    assert!(load(&f.db, &first.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    let old_challenge = f.issue(SessionStage::MfaRequired).await;
    assert!(matches!(
        f.service
            .verify_mfa(
                &old_challenge.session,
                &old_codes[0],
                CodeType::Recovery,
                peer(),
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));

    let a = f.issue(SessionStage::MfaRequired).await;
    let b = f.issue(SessionStage::MfaRequired).await;
    let normalized = format!("  {}  ", codes[0].replace('-', "").to_ascii_lowercase());
    let (a, b) = tokio::join!(
        f.service
            .verify_mfa(&a.session, &normalized, CodeType::Recovery, peer()),
        f.service
            .verify_mfa(&b.session, &codes[0], CodeType::Recovery, peer()),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(AuthError::InvalidVerificationCode))
            | (Err(AuthError::InvalidVerificationCode), Ok(_))
    ));
    let hash = totp::recovery_hash(&f.user_id, &codes[0]).unwrap();
    assert!(f
        .db
        .query_one(
            "SELECT used_at IS NOT NULL FROM auth_recovery_codes WHERE user_id=$1 AND code_hash=$2",
            &[&f.user_id.as_str(), &hash],
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    let challenge = f.issue(SessionStage::MfaRequired).await;
    assert!(matches!(
        f.service
            .verify_mfa(&challenge.session, &codes[0], CodeType::Recovery, peer(),)
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    assert_eq!(
        f.db.query_one(
            "SELECT count(*) FROM auth_recovery_codes WHERE user_id=$1",
            &[&f.user_id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        10
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_password_change_revokes_other_authority_and_preserves_deadline_and_proof() {
    let mut f = Fixture::new(false).await;
    let original = f.issue(SessionStage::Authenticated).await;
    let other = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let (source, _) = f
        .service
        .totp_start(&original.session, Some(&actor), peer())
        .await
        .unwrap();
    let token_id = uuid::Uuid::new_v4().to_string();
    let link_id = uuid::Uuid::new_v4().to_string();
    f.db.execute(
        "INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,created_at,expires_at) VALUES($1::text::uuid,$2,'fixture-token-hash','fixture','read',$3,now(),now()+INTERVAL '1 day')",
        &[&token_id, &f.user_id.as_str(), &actor.auth_version],
    ).await.unwrap();
    f.db.execute(
        "UPDATE trading_connection SET agent_token_hash='fixture-agent-hash',agent_auth_version=$2 WHERE user_id=$1",
        &[&f.user_id.as_str(), &actor.auth_version],
    ).await.unwrap();
    f.db.execute(
        "INSERT INTO auth_links(id,token_hash,kind,user_id,target_auth_version,created_at,expires_at) VALUES($1,'fixture-link-hash','password_reset',$2,$3,now(),now()+INTERVAL '1 hour')",
        &[&link_id, &f.user_id.as_str(), &actor.auth_version],
    ).await.unwrap();
    let replacement = f
        .service
        .change_password(&actor, &source, "changed fixture password".into())
        .await
        .unwrap();
    assert_eq!(
        replacement.session.auth_version,
        Some(actor.auth_version + 1)
    );
    assert_eq!(
        replacement.session.absolute_expires_at,
        original.session.absolute_expires_at
    );
    assert_eq!(
        replacement.session.reauthenticated_at,
        original.session.reauthenticated_at
    );
    assert!(replacement.session.pending_totp.is_none());
    assert_ne!(replacement.session.csrf_token, source.csrf_token);
    for cookie in [&original.raw_cookie, &other.raw_cookie] {
        assert!(load(&f.db, cookie, Utc::now()).await.unwrap().is_none());
    }
    assert!(load(&f.db, &replacement.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NOT NULL FROM auth_api_tokens WHERE id::text=$1",
            &[&token_id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert!(f.db.query_one("SELECT agent_token_hash IS NULL AND agent_auth_version IS NULL FROM trading_connection WHERE user_id=$1", &[&f.user_id.as_str()])
        .await.unwrap().get::<_, bool>(0));
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NOT NULL FROM auth_links WHERE id=$1",
            &[&link_id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    let user = f.user().await;
    assert!(
        f.service
            .passwords
            .verify(
                "changed fixture password".into(),
                user.password_hash.clone()
            )
            .await
            .unwrap()
            .valid
    );
    assert!(
        !f.service
            .passwords
            .verify(PASSWORD.into(), user.password_hash)
            .await
            .unwrap()
            .valid
    );
    assert!(matches!(
        f.service
            .regenerate_recovery_codes(&actor, &replacement.session)
            .await,
        Err(AuthError::StateChanged)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_deleted_source_cannot_revive_through_any_credential_change() {
    let mut f = Fixture::new(false).await;
    let full = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let before = f.user().await;
    let (source, enrollment) = f
        .service
        .totp_start(&full.session, Some(&actor), peer())
        .await
        .unwrap();
    f.db.execute(
        "DELETE FROM auth_sessions WHERE id_hash=$1",
        &[&source.id_hash],
    )
    .await
    .unwrap();
    assert!(matches!(
        f.service.totp_start(&source, Some(&actor), peer()).await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service
            .totp_cancel(&source, Some(&actor), &enrollment.enrollment_id)
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service
            .totp_confirm(
                &source,
                Some(&actor),
                &enrollment.enrollment_id,
                &enrollment_code(&enrollment),
                peer(),
            )
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service
            .change_password(&actor, &source, "cannot revive password".into())
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service.regenerate_recovery_codes(&actor, &source).await,
        Err(AuthError::StateChanged)
    ));
    let user = f.user().await;
    assert_eq!(user.auth_version, before.auth_version);
    assert_eq!(user.password_hash, before.password_hash);
    assert_eq!(user.totp_secret_encrypted, before.totp_secret_encrypted);
    assert_eq!(
        f.db.query_one(
            "SELECT count(*) FROM auth_sessions WHERE user_id=$1",
            &[&f.user_id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_recent_browser_actor_is_required_for_every_full_credential_mutation() {
    let mut f = Fixture::new(false).await;
    let full = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let (source, enrollment) = f
        .service
        .totp_start(&full.session, Some(&actor), peer())
        .await
        .unwrap();
    f.db.execute(
        "UPDATE auth_sessions SET reauthenticated_at=$2 WHERE id_hash=$1",
        &[&source.id_hash, &(Utc::now() - Duration::minutes(6))],
    )
    .await
    .unwrap();
    let old = f.stored(&source).await;
    assert!(matches!(
        f.service.totp_start(&old, Some(&actor), peer()).await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .totp_cancel(&old, Some(&actor), &enrollment.enrollment_id)
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .totp_confirm(
                &old,
                Some(&actor),
                &enrollment.enrollment_id,
                &enrollment_code(&enrollment),
                peer()
            )
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .change_password(&actor, &old, "valid but not recent".into())
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service.regenerate_recovery_codes(&actor, &old).await,
        Err(AuthError::ReauthenticationRequired)
    ));
    let fresh = f.issue(SessionStage::Authenticated).await;
    assert!(matches!(
        f.service.totp_start(&fresh.session, None, peer()).await,
        Err(AuthError::Unauthenticated)
    ));
    let mut token_actor = actor.clone();
    token_actor.credential = CredentialKind::PersonalWriteToken;
    assert!(matches!(
        f.service
            .totp_start(&fresh.session, Some(&token_actor), peer())
            .await,
        Err(AuthError::Forbidden)
    ));
    let mut stale_role = actor.clone();
    stale_role.role = crate::database::models::auth::Role::Member;
    assert!(matches!(
        f.service
            .regenerate_recovery_codes(&stale_role, &fresh.session)
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(f.user().await.status == AccountStatus::Active);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_initial_enrollment_activates_only_on_confirmation_and_denies_replay() {
    let mut f = Fixture::new(true).await;
    let enrollment = f.issue(SessionStage::EnrollmentRequired).await;
    let other = f.issue(SessionStage::EnrollmentRequired).await;
    let before = f.user().await;
    let (source, response) = f
        .service
        .totp_start(&enrollment.session, None, peer())
        .await
        .unwrap();
    assert!(f.user().await.status == AccountStatus::Pending);
    assert!(f.user().await.totp_secret_encrypted.is_none());
    assert!(source.stage == SessionStage::EnrollmentRequired);
    let code = enrollment_code(&response);
    let started = Utc::now();
    let (full, codes) = f
        .service
        .totp_confirm(&source, None, &response.enrollment_id, &code, peer())
        .await
        .unwrap();
    let user = f.user().await;
    assert!(user.fully_enrolled());
    assert_eq!(user.auth_version, before.auth_version + 1);
    assert!(full.session.stage == SessionStage::Authenticated);
    assert!(full.session.absolute_expires_at >= started + Duration::hours(12));
    assert!(full.session.absolute_expires_at <= Utc::now() + Duration::hours(12));
    assert!(full
        .session
        .reauthenticated_at
        .is_some_and(|proof| proof >= started));
    assert!(full.session.pending_totp.is_none());
    assert_eq!(codes.len(), 10);
    assert!(load(&f.db, &enrollment.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(load(&f.db, &other.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(load(&f.db, &full.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    let encrypted = user.totp_secret_encrypted.as_deref().unwrap();
    assert!(matches!(
        f.service.decrypt_totp(&user.id, true, encrypted),
        Err(AuthError::Unavailable)
    ));
    let active = f.service.decrypt_totp(&user.id, false, encrypted).unwrap();
    assert_eq!(
        user.last_totp_step,
        totp::verify_totp_step(&active, &code, Utc::now().timestamp() as u64, None),
    );
    let challenge = f.issue(SessionStage::MfaRequired).await;
    assert!(matches!(
        f.service
            .verify_mfa(&challenge.session, &code, CodeType::Totp, peer(),)
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_credential_commit_failures_return_no_success_and_rollback_authority() {
    let mut f = Fixture::new(false).await;
    let full = f.issue(SessionStage::Authenticated).await;
    let other = f.issue(SessionStage::Authenticated).await;
    let actor = f.actor().await;
    let before = f.user().await;
    let (source, enrollment) = f
        .service
        .totp_start(&full.session, Some(&actor), peer())
        .await
        .unwrap();
    // A deferred database failure exercises the actual commit path, after all
    // account/code/successor writes have been accepted by PostgreSQL.
    f.db.batch_execute(
        "CREATE FUNCTION reject_credential_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'isolated credential commit failure'; END $$;
         CREATE CONSTRAINT TRIGGER reject_credential_commit
         AFTER INSERT OR UPDATE ON auth_sessions DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION reject_credential_commit();",
    )
    .await
    .unwrap();
    assert!(matches!(
        f.service.totp_start(&source, Some(&actor), peer()).await,
        Err(AuthError::Unavailable)
    ));
    assert!(matches!(
        f.service
            .totp_cancel(&source, Some(&actor), &enrollment.enrollment_id)
            .await,
        Err(AuthError::Unavailable)
    ));
    assert!(matches!(
        f.service
            .totp_confirm(
                &source,
                Some(&actor),
                &enrollment.enrollment_id,
                &enrollment_code(&enrollment),
                peer(),
            )
            .await,
        Err(AuthError::Unavailable)
    ));
    assert!(matches!(
        f.service
            .change_password(&actor, &source, "must roll back password".into(),)
            .await,
        Err(AuthError::Unavailable)
    ));
    assert!(matches!(
        f.service.regenerate_recovery_codes(&actor, &source).await,
        Err(AuthError::Unavailable)
    ));
    let stored = f.stored(&source).await;
    assert_eq!(stored.revision, source.revision);
    assert_eq!(stored.pending_totp.unwrap().id, enrollment.enrollment_id);
    let user = f.user().await;
    assert_eq!(user.auth_version, before.auth_version);
    assert_eq!(user.password_hash, before.password_hash);
    assert_eq!(user.totp_secret_encrypted, before.totp_secret_encrypted);
    f.db.batch_execute(
        "DROP TRIGGER reject_credential_commit ON auth_sessions;
         DROP FUNCTION reject_credential_commit();",
    )
    .await
    .unwrap();
    assert!(load(&f.db, &full.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    assert!(load(&f.db, &other.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        f.db.query_one(
            "SELECT count(*) FROM auth_recovery_codes WHERE user_id=$1",
            &[&f.user_id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    assert!(f
        .service
        .totp_confirm(
            &source,
            Some(&actor),
            &enrollment.enrollment_id,
            &enrollment_code(&enrollment),
            peer(),
        )
        .await
        .is_ok());
    f.cleanup().await;
}
