use std::{ffi::OsString, sync::Arc};

use tokio_postgres::Client;

use super::*;
use crate::services::auth::{
    session_store::{create_anonymous, load},
    tests::{session_created_at, test_database_at},
    totp::recovery_hash,
    AuthConfig,
};

const PASSWORD: &str = "a complete test password";
const NEW_PASSWORD: &str = "a replacement test password";
const RECOVERY_CODE: &str = "01234567-89ABCDEF-01234567-89ABCDEF";
const SECRET: &[u8] = b"12345678901234567890";

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
        let service = AuthService::new(
            AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
        );
        Self {
            client,
            schema,
            previous_url,
            service,
        }
    }

    async fn anonymous(&mut self) -> IssuedSession {
        let tx = self.client.transaction().await.unwrap();
        let issued = create_anonymous(&tx, Utc::now()).await.unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn user(&self, username: &str) -> AuthUser {
        let id = UserId::new();
        let hash = self.service.passwords.hash(PASSWORD.into()).await.unwrap();
        let factor = self.service.encrypt_totp(&id, false, SECRET).unwrap();
        self.client.execute(
            "INSERT INTO auth_users(id,username,password_hash,role,status,totp_secret_encrypted) VALUES($1,$2,$3,'member','active',$4)",
            &[&id.as_str(), &username, &hash, &factor],
        ).await.unwrap();
        auth::by_id(&id, &self.client).await.unwrap().unwrap()
    }

    async fn issue(
        &mut self,
        kind: AccountLinkKind,
        target: Option<&AuthUser>,
    ) -> IssuedAccountLink {
        let tx = self.client.transaction().await.unwrap();
        if kind == AccountLinkKind::Bootstrap {
            auth::lock_instance(&tx).await.unwrap();
        }
        if let Some(target) = target {
            auth::lock_user(&target.id, &tx).await.unwrap();
        }
        let now = Utc::now();
        let issued = issue_link_locked(
            &tx,
            &self.service,
            kind,
            target,
            None,
            now,
            now + Duration::hours(1),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn bind(&mut self, link: &IssuedAccountLink) -> AuthSession {
        let anonymous = self.anonymous().await;
        self.service
            .inspect_link(&anonymous.session, token(link), peer())
            .await
            .unwrap()
            .0
    }

    async fn recovery(&self, user: &AuthUser) {
        let hash = recovery_hash(&user.id, RECOVERY_CODE).unwrap();
        self.client
            .execute(
                "INSERT INTO auth_recovery_codes(user_id,code_hash) VALUES($1,$2)",
                &[&user.id.as_str(), &hash],
            )
            .await
            .unwrap();
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

fn peer() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}
fn token(link: &IssuedAccountLink) -> &str {
    link.url.split_once("#token=").unwrap().1
}
fn error<T>(result: Result<T, AuthError>) -> AuthError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("unexpected account-link success"),
    }
}

#[test]
fn requests_reject_unknown_fields_and_keep_factor_codes_as_strings() {
    assert!(serde_json::from_value::<InspectLinkRequest>(
        serde_json::json!({"token":"link","user_id":"owner"})
    )
    .is_err());
    assert!(serde_json::from_value::<RegisterRequest>(
        serde_json::json!({"link_id":"link","username":"alice","password":PASSWORD,"role":"admin"})
    )
    .is_err());
    assert!(serde_json::from_value::<RecoverRequest>(
        serde_json::json!({"link_id":"link","new_password":PASSWORD,"enable":true})
    )
    .is_err());
    assert!(serde_json::from_value::<PasswordResetRequest>(serde_json::json!({"link_id":"link","new_password":PASSWORD,"code":"081804","code_type":"totp","disable_totp":true})).is_err());
    let request: PasswordResetRequest = serde_json::from_value(serde_json::json!({"link_id":"link","new_password":PASSWORD,"code":"081804","code_type":"totp"})).unwrap();
    assert_eq!(request.code, "081804");
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_bootstrap_has_one_winner_and_preserves_reserved_controls() {
    let mut db = Database::new().await;
    let owner_id = auth::legacy_owner(&db.client).await.unwrap();
    let owner = auth::by_id(&owner_id, &db.client).await.unwrap().unwrap();
    db.client.execute("UPDATE user_notification_settings SET telegram_chat_id='legacy-destination' WHERE user_id=$1", &[&owner_id.as_str()]).await.unwrap();
    db.client
        .execute(
            "UPDATE telegram_report_schedule SET send_time_utc='07:00' WHERE user_id=$1",
            &[&owner_id.as_str()],
        )
        .await
        .unwrap();
    let link = db.issue(AccountLinkKind::Bootstrap, Some(&owner)).await;
    assert!(link
        .url
        .starts_with("https://auth.example.test/register#token=doughbox_link_"));
    let stored: String = db
        .client
        .query_one("SELECT token_hash FROM auth_links WHERE id=$1", &[&link.id])
        .await
        .unwrap()
        .get(0);
    assert_ne!(stored, token(&link));
    let first = db.bind(&link).await;
    let second = db.bind(&link).await;
    let (a, b) = tokio::join!(
        db.service.register(
            &first,
            &link.id,
            " First.Owner ".into(),
            PASSWORD.into(),
            peer()
        ),
        db.service.register(
            &second,
            &link.id,
            "other.owner".into(),
            PASSWORD.into(),
            peer()
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (winner, loser) = match (a, b) {
        (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
        _ => panic!("bootstrap race did not have one winner"),
    };
    assert!(matches!(loser, AuthError::InvalidAccountLink));
    assert!(winner.session.stage == SessionStage::EnrollmentRequired);
    assert_eq!(winner.session.user_id.as_ref(), Some(&owner_id));
    let owner = auth::by_id(&owner_id, &db.client).await.unwrap().unwrap();
    assert!(owner.role == Role::Admin && owner.status == AccountStatus::Pending);
    assert!(owner.totp_secret_encrypted.is_none());
    let row = db.client.query_one("SELECT n.telegram_chat_id,s.send_time_utc FROM user_notification_settings n JOIN telegram_report_schedule s USING(user_id) WHERE n.user_id=$1", &[&owner_id.as_str()]).await.unwrap();
    assert_eq!(row.get::<_, String>(0), "legacy-destination");
    assert_eq!(row.get::<_, String>(1), "07:00");
    let anonymous = db.anonymous().await;
    assert!(matches!(
        db.service
            .inspect_link(&anonymous.session, token(&link), peer())
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    assert!(matches!(
        db.service
            .register(
                &anonymous.session,
                &link.id,
                "visitor".into(),
                PASSWORD.into(),
                peer()
            )
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_invitation_collision_does_not_burn_link_and_parallel_redemption_has_one_winner(
) {
    let mut db = Database::new().await;
    db.user("taken.name").await;
    let link = db.issue(AccountLinkKind::Invite, None).await;
    let first = db.bind(&link).await;
    assert!(matches!(
        db.service
            .register(
                &first,
                &link.id,
                " TAKEN.NAME ".into(),
                PASSWORD.into(),
                peer()
            )
            .await,
        Err(AuthError::UsernameUnavailable)
    ));
    let second = db.bind(&link).await;
    let (a, b) = tokio::join!(
        db.service.register(
            &first,
            &link.id,
            "member.one".into(),
            PASSWORD.into(),
            peer()
        ),
        db.service.register(
            &second,
            &link.id,
            "member.two".into(),
            PASSWORD.into(),
            peer()
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let winner = a.ok().or_else(|| b.ok()).unwrap();
    let id = winner.session.user_id.as_ref().unwrap();
    let user = auth::by_id(id, &db.client).await.unwrap().unwrap();
    assert!(user.status == AccountStatus::Pending && user.role == Role::Member);
    assert!(winner.session.stage == SessionStage::EnrollmentRequired);
    let count: i64 = db
        .client
        .query_one(
            "SELECT count(*) FROM calculation_status WHERE user_id=$1",
            &[&id.as_str()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 2);
    let row = db.client.query_one("SELECT s.enabled,n.telegram_chat_id,c.credentials,c.agent_token_hash FROM telegram_report_schedule s JOIN user_notification_settings n USING(user_id) JOIN trading_connection c USING(user_id) WHERE s.user_id=$1", &[&id.as_str()]).await.unwrap();
    assert!(!row.get::<_, bool>(0));
    for column in 1..4 {
        assert!(row.get::<_, Option<String>>(column).is_none());
    }
    let anonymous = db.anonymous().await;
    let resumed = db
        .service
        .login(
            &anonymous.session,
            user.username.unwrap(),
            PASSWORD.into(),
            peer(),
        )
        .await
        .unwrap();
    assert!(resumed.session.stage == SessionStage::EnrollmentRequired);
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_inspection_binding_rejects_tab_mix_stale_revision_and_expired_binding() {
    let mut db = Database::new().await;
    let first_link = db.issue(AccountLinkKind::Invite, None).await;
    let second_link = db.issue(AccountLinkKind::Invite, None).await;
    let anonymous = db.anonymous().await;
    let (first, view) = db
        .service
        .inspect_link(&anonymous.session, token(&first_link), peer())
        .await
        .unwrap();
    assert!(view.username.is_none());
    assert!(view.expires_at <= first_link.expires_at);
    assert_eq!(first.revision, anonymous.session.revision + 1);
    let (second, _) = db
        .service
        .inspect_link(&first, token(&second_link), peer())
        .await
        .unwrap();
    assert!(matches!(
        db.service
            .register(
                &second,
                &first_link.id,
                "wrong.tab".into(),
                PASSWORD.into(),
                peer()
            )
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    assert!(matches!(
        db.service
            .register(
                &first,
                &first_link.id,
                "stale.tab".into(),
                PASSWORD.into(),
                peer()
            )
            .await,
        Err(AuthError::StateChanged)
    ));
    db.client
        .execute(
            "UPDATE auth_sessions SET link_bound_until=now()-interval '1 second' WHERE id_hash=$1",
            &[&second.id_hash],
        )
        .await
        .unwrap();
    let expired = load(&db.client, &anonymous.raw_cookie, Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        db.service
            .register(
                &expired,
                &second_link.id,
                "expired.tab".into(),
                PASSWORD.into(),
                peer()
            )
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    assert!(matches!(
        db.service
            .inspect_link(&expired, "not-a-link", peer())
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_links_deny_expiry_revocation_version_changes_and_logged_out_sources() {
    let mut db = Database::new().await;
    for mutation in ["expiry", "revocation", "version", "logout"] {
        let user = db.user(&format!("alice.{mutation}")).await;
        let link = db.issue(AccountLinkKind::PasswordReset, Some(&user)).await;
        let source = db.bind(&link).await;
        match mutation {
            "expiry" => {
                db.client
                    .execute(
                        "UPDATE auth_links SET expires_at=now()-interval '1 second' WHERE id=$1",
                        &[&link.id],
                    )
                    .await
                    .unwrap();
            }
            "revocation" => {
                db.client
                    .execute(
                        "UPDATE auth_links SET revoked_at=now() WHERE id=$1",
                        &[&link.id],
                    )
                    .await
                    .unwrap();
            }
            "version" => {
                db.client
                    .execute(
                        "UPDATE auth_users SET auth_version=auth_version+1 WHERE id=$1",
                        &[&user.id.as_str()],
                    )
                    .await
                    .unwrap();
            }
            "logout" => {
                db.service.logout(&source).await.unwrap();
            }
            _ => unreachable!(),
        }
        let denied = error(
            db.service
                .reset_password(
                    &source,
                    &link.id,
                    NEW_PASSWORD.into(),
                    RECOVERY_CODE,
                    CodeType::Recovery,
                    peer(),
                )
                .await,
        );
        assert!(matches!(
            denied,
            AuthError::InvalidAccountLink | AuthError::StateChanged
        ));
        if mutation != "logout" {
            let anonymous = db.anonymous().await;
            assert!(matches!(
                db.service
                    .inspect_link(&anonymous.session, token(&link), peer())
                    .await,
                Err(AuthError::InvalidAccountLink)
            ));
        }
    }
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_password_reset_requires_existing_factor_and_revokes_old_authority() {
    let mut db = Database::new().await;
    let user = db.user("alice").await;
    db.recovery(&user).await;
    let anonymous = db.anonymous().await;
    let challenge = db
        .service
        .login(&anonymous.session, "alice".into(), PASSWORD.into(), peer())
        .await
        .unwrap();
    let link = db.issue(AccountLinkKind::PasswordReset, Some(&user)).await;
    let other_link = db.issue(AccountLinkKind::PasswordReset, Some(&user)).await;
    assert!(link
        .url
        .starts_with("https://auth.example.test/recover#token=doughbox_link_"));
    let source = db.bind(&link).await;
    let anonymous = db.anonymous().await;
    let (_, view) = db
        .service
        .inspect_link(&anonymous.session, token(&link), peer())
        .await
        .unwrap();
    assert_eq!(view.username.as_deref(), Some("alice"));
    assert!(matches!(
        db.service
            .reset_password(
                &source,
                &link.id,
                NEW_PASSWORD.into(),
                "not-a-code",
                CodeType::Totp,
                peer()
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    let unchanged = auth::by_id(&user.id, &db.client).await.unwrap().unwrap();
    assert_eq!(unchanged.auth_version, user.auth_version);
    assert_eq!(unchanged.password_hash, user.password_hash);
    let id = uuid::Uuid::new_v4().to_string();
    db.client.execute("INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,expires_at) VALUES($1::text::uuid,$2,'reset-test-token','test','read',$3,now()+interval '1 day')", &[&id, &user.id.as_str(), &user.auth_version]).await.unwrap();
    let reset = db
        .service
        .reset_password(
            &source,
            &link.id,
            NEW_PASSWORD.into(),
            RECOVERY_CODE,
            CodeType::Recovery,
            peer(),
        )
        .await
        .unwrap();
    assert!(reset.session.stage == SessionStage::Authenticated);
    assert_eq!(reset.session.auth_version, Some(user.auth_version + 1));
    assert!(reset.session.reauthenticated_at.is_some());
    assert_eq!(
        reset.session.absolute_expires_at - session_created_at(&db.client, &reset.session).await,
        Duration::hours(12)
    );
    assert!(reset.session.link_id.is_none() && reset.session.pending_totp.is_none());
    let changed = auth::by_id(&user.id, &db.client).await.unwrap().unwrap();
    assert_eq!(changed.totp_secret_encrypted, user.totp_secret_encrypted);
    assert!(changed.status == AccountStatus::Active);
    assert!(
        db.service
            .passwords
            .verify(NEW_PASSWORD.into(), changed.password_hash)
            .await
            .unwrap()
            .valid
    );
    assert!(load(&db.client, &challenge.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(db
        .client
        .query_one(
            "SELECT revoked_at IS NOT NULL FROM auth_api_tokens WHERE id::text=$1",
            &[&id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    let anonymous = db.anonymous().await;
    for link in [&link, &other_link] {
        assert!(matches!(
            db.service
                .inspect_link(&anonymous.session, token(link), peer())
                .await,
            Err(AuthError::InvalidAccountLink)
        ));
    }
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_password_reset_cannot_reactivate_disabled_target_or_bypass_factor_replay() {
    let mut db = Database::new().await;
    let user = db.user("alice").await;
    db.recovery(&user).await;
    let link = db.issue(AccountLinkKind::PasswordReset, Some(&user)).await;
    let source = db.bind(&link).await;
    db.client
        .execute(
            "UPDATE auth_users SET status='disabled' WHERE id=$1",
            &[&user.id.as_str()],
        )
        .await
        .unwrap();
    assert!(matches!(
        db.service
            .reset_password(
                &source,
                &link.id,
                NEW_PASSWORD.into(),
                RECOVERY_CODE,
                CodeType::Recovery,
                peer()
            )
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    let disabled = auth::by_id(&user.id, &db.client).await.unwrap().unwrap();
    assert!(disabled.status == AccountStatus::Disabled);
    assert_eq!(disabled.password_hash, user.password_hash);
    db.client
        .execute(
            "UPDATE auth_users SET status='active' WHERE id=$1",
            &[&user.id.as_str()],
        )
        .await
        .unwrap();
    let code_hash = recovery_hash(&user.id, RECOVERY_CODE).unwrap();
    db.client
        .execute(
            "UPDATE auth_recovery_codes SET used_at=now() WHERE user_id=$1 AND code_hash=$2",
            &[&user.id.as_str(), &code_hash],
        )
        .await
        .unwrap();
    assert!(matches!(
        db.service
            .reset_password(
                &source,
                &link.id,
                NEW_PASSWORD.into(),
                RECOVERY_CODE,
                CodeType::Recovery,
                peer()
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_operator_link_redemption_is_single_use_and_enrollment_can_resume() {
    let mut db = Database::new().await;
    let user = db.user("alice").await;
    // Console recovery has already invalidated old authority and cleared both factors.
    db.client.execute("UPDATE auth_users SET status='pending',password_hash=NULL,totp_secret_encrypted=NULL,last_totp_step=NULL,auth_version=auth_version+1 WHERE id=$1", &[&user.id.as_str()]).await.unwrap();
    let pending = auth::by_id(&user.id, &db.client).await.unwrap().unwrap();
    let link = db
        .issue(AccountLinkKind::OperatorRecovery, Some(&pending))
        .await;
    let source = db.bind(&link).await;
    let second = db.bind(&link).await;
    let (a, b) = tokio::join!(
        db.service
            .recover_account(&source, &link.id, NEW_PASSWORD.into(), peer()),
        db.service
            .recover_account(&second, &link.id, NEW_PASSWORD.into(), peer()),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let recovered = a.ok().or_else(|| b.ok()).unwrap();
    assert!(recovered.session.stage == SessionStage::EnrollmentRequired);
    assert_eq!(recovered.session.auth_version, Some(pending.auth_version));
    assert!(recovered.session.reauthenticated_at.is_none());
    let user = auth::by_id(&user.id, &db.client).await.unwrap().unwrap();
    assert_eq!(user.username.as_deref(), Some("alice"));
    assert!(user.status == AccountStatus::Pending && user.totp_secret_encrypted.is_none());
    assert!(matches!(
        db.service
            .verify_mfa(
                &recovered.session,
                RECOVERY_CODE,
                CodeType::Recovery,
                peer()
            )
            .await,
        Err(AuthError::Forbidden)
    ));
    let anonymous = db.anonymous().await;
    assert!(matches!(
        db.service
            .inspect_link(&anonymous.session, token(&link), peer())
            .await,
        Err(AuthError::InvalidAccountLink)
    ));
    let resumed = db
        .service
        .login(
            &anonymous.session,
            "alice".into(),
            NEW_PASSWORD.into(),
            peer(),
        )
        .await
        .unwrap();
    assert!(resumed.session.stage == SessionStage::EnrollmentRequired);
    let enrollment = db
        .service
        .totp_start(&resumed.session, None, peer())
        .await
        .unwrap();
    assert!(enrollment.0.pending_totp.is_some());
    assert!(!enrollment.1.secret_base32.is_empty());
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_password_reset_totp_has_one_winner_and_remains_replay_protected() {
    let mut db = Database::new().await;
    let user = db.user("alice").await;
    let first_link = db.issue(AccountLinkKind::PasswordReset, Some(&user)).await;
    let second_link = db.issue(AccountLinkKind::PasswordReset, Some(&user)).await;
    let first = db.bind(&first_link).await;
    let second = db.bind(&second_link).await;
    let code = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        0,
        30,
        SECRET.to_vec(),
        None,
        String::new(),
    )
    .unwrap()
    .generate(Utc::now().timestamp() as u64);
    let (a, b) = tokio::join!(
        db.service.reset_password(
            &first,
            &first_link.id,
            NEW_PASSWORD.into(),
            &code,
            CodeType::Totp,
            peer()
        ),
        db.service.reset_password(
            &second,
            &second_link.id,
            NEW_PASSWORD.into(),
            &code,
            CodeType::Totp,
            peer()
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let current = auth::by_id(&user.id, &db.client).await.unwrap().unwrap();
    let replay_link = db
        .issue(AccountLinkKind::PasswordReset, Some(&current))
        .await;
    let replay = db.bind(&replay_link).await;
    assert!(matches!(
        db.service
            .reset_password(
                &replay,
                &replay_link.id,
                PASSWORD.into(),
                &code,
                CodeType::Totp,
                peer()
            )
            .await,
        Err(AuthError::InvalidVerificationCode)
    ));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_registration_commit_failure_retains_link_and_source_authority() {
    let mut db = Database::new().await;
    let link = db.issue(AccountLinkKind::Invite, None).await;
    let source = db.bind(&link).await;
    db.client.batch_execute(
        "CREATE FUNCTION fail_link_enrollment_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
           IF NEW.stage='enrollment_required' THEN RAISE EXCEPTION 'test commit failure'; END IF;
           RETURN NEW;
         END $$;
         CREATE CONSTRAINT TRIGGER fail_link_enrollment_commit AFTER INSERT ON auth_sessions
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_link_enrollment_commit();"
    ).await.unwrap();
    assert!(matches!(
        db.service
            .register(&source, &link.id, "alice".into(), PASSWORD.into(), peer())
            .await,
        Err(AuthError::Unavailable)
    ));
    assert!(auth::by_username(&db.client, "alice")
        .await
        .unwrap()
        .is_none());
    db.client.batch_execute("DROP TRIGGER fail_link_enrollment_commit ON auth_sessions; DROP FUNCTION fail_link_enrollment_commit()").await.unwrap();
    let registered = db
        .service
        .register(&source, &link.id, "alice".into(), PASSWORD.into(), peer())
        .await
        .unwrap();
    assert!(registered.session.stage == SessionStage::EnrollmentRequired);
    db.cleanup().await;
}
