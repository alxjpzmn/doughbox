use std::{ffi::OsString, sync::Arc};

use chrono::{Duration, Utc};
use tokio_postgres::Client;

use super::*;
use crate::{
    database::models::auth::{CredentialKind, UserId},
    services::auth::{
        session_store::{self, IssuedSession, SessionGrant},
        tests::{cleanup, test_database_at},
        AuthConfig,
    },
};

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
            Some(value) => std::env::set_var("POSTGRES_URL", value),
            None => std::env::remove_var("POSTGRES_URL"),
        }
    }
}

pub(crate) struct Fixture {
    pub(crate) db: Client,
    schema: String,
    pub(crate) service: Arc<AuthService>,
    pub(crate) admin: CurrentUser,
    pub(crate) source: AuthSession,
    _url: ScopedDatabaseUrl,
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let url = ScopedDatabaseUrl::new(&schema);
        let service = AuthService::new(
            AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
        );
        let id = auth::legacy_owner(&db).await.unwrap();
        db.execute("UPDATE auth_users SET username='alice',password_hash='fixture-phc',totp_secret_encrypted='fixture-envelope',status='active' WHERE id=$1", &[&id.as_str()]).await.unwrap();
        let user = auth::by_id(&id, &db).await.unwrap().unwrap();
        let admin = user.current(CredentialKind::BrowserSession).unwrap();
        let source = Self::issue(&mut db, &user).await.session;
        Self {
            db,
            schema,
            service,
            admin,
            source,
            _url: url,
        }
    }

    async fn issue(db: &mut Client, user: &AuthUser) -> IssuedSession {
        let now = Utc::now();
        let tx = db.transaction().await.unwrap();
        let anon = session_store::create_anonymous(&tx, now).await.unwrap();
        let session = session_store::issue_successor_locked(
            &tx,
            &anon.session,
            SessionGrant::full(user, now),
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        session
    }

    async fn add(&mut self, username: &str, role: Role, complete: bool) -> AuthUser {
        let id = UserId::new();
        let status = if complete { "active" } else { "pending" };
        let secret = complete.then_some("other-fixture-envelope");
        self.db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,$2,'other-fixture-phc',$3,$4,$5)", &[&id.as_str(), &username, &secret, &role.as_str(), &status]).await.unwrap();
        let tx = self.db.transaction().await.unwrap();
        auth::initialize_private_defaults(&id, &tx).await.unwrap();
        tx.commit().await.unwrap();
        auth::by_id(&id, &self.db).await.unwrap().unwrap()
    }

    async fn user(&self, id: &UserId) -> AuthUser {
        auth::by_id(id, &self.db).await.unwrap().unwrap()
    }

    pub(crate) async fn fail_link_commit(&self) {
        self.db.batch_execute("CREATE FUNCTION reject_admin_link_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test commit failure'; END $$;
            CREATE CONSTRAINT TRIGGER reject_admin_link_commit AFTER INSERT ON auth_links DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_admin_link_commit();").await.unwrap();
    }

    pub(crate) async fn cleanup(self) {
        cleanup(self.db, &self.schema).await;
    }
}

fn patch(role: Option<Role>, disabled: Option<bool>) -> UserPatchRequest {
    UserPatchRequest { role, disabled }
}

#[test]
fn administrator_patch_rejects_owner_username_and_unknown_fields() {
    for value in [
        serde_json::json!({"username":"new-name"}),
        serde_json::json!({"user_id":"other"}),
        serde_json::json!({"totp_enabled":false}),
    ] {
        assert!(serde_json::from_value::<UserPatchRequest>(value).is_err());
    }
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_captured_actor_source_and_recent_authority_are_required() {
    let mut f = Fixture::new().await;
    let target = f.add("bob", Role::Member, true).await;
    let invite = f.service.create_invite(&f.admin, &f.source).await.unwrap();
    let mut token_actor = f.admin.clone();
    token_actor.credential = CredentialKind::PersonalWriteToken;
    assert!(matches!(
        f.service.list_users(&token_actor, &f.source).await,
        Err(AuthError::Forbidden)
    ));
    token_actor.credential = CredentialKind::PersonalReadToken;
    assert!(matches!(
        f.service.create_invite(&token_actor, &f.source).await,
        Err(AuthError::Forbidden)
    ));
    let member = target.current(CredentialKind::BrowserSession).unwrap();
    let member_source = Fixture::issue(&mut f.db, &target).await;
    assert!(matches!(
        f.service
            .list_invites(&member, &member_source.session)
            .await,
        Err(AuthError::Forbidden)
    ));
    assert!(matches!(
        f.service
            .create_invite(&f.admin, &member_source.session)
            .await,
        Err(AuthError::StateChanged)
    ));
    let mut stale_actor = f.admin.clone();
    stale_actor.auth_version += 1;
    let mut stale_source = f.source.clone();
    stale_source.auth_version = Some(stale_actor.auth_version);
    assert!(matches!(
        f.service
            .update_user(
                &stale_actor,
                &stale_source,
                target.id.as_str(),
                patch(None, Some(true))
            )
            .await,
        Err(AuthError::StateChanged)
    ));
    f.db.execute(
        "UPDATE auth_sessions SET reauthenticated_at=$2 WHERE id_hash=$1",
        &[&f.source.id_hash, &(Utc::now() - Duration::minutes(6))],
    )
    .await
    .unwrap();
    assert!(f.service.list_users(&f.admin, &f.source).await.is_ok());
    let summary =
        serde_json::to_value(f.service.list_users(&f.admin, &f.source).await.unwrap()).unwrap();
    let fields = summary["users"][0].as_object().unwrap();
    assert_eq!(fields.len(), 6);
    for key in [
        "id",
        "username",
        "role",
        "status",
        "totp_enabled",
        "created_at",
    ] {
        assert!(fields.contains_key(key));
    }
    assert!(matches!(
        f.service.create_invite(&f.admin, &f.source).await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .update_user(&f.admin, &f.source, target.id.as_str(), patch(None, None))
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .revoke_user_sessions(&f.admin, &f.source, target.id.as_str())
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .password_reset_link(&f.admin, &f.source, target.id.as_str())
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    assert!(matches!(
        f.service
            .revoke_invite(&f.admin, &f.source, &invite.id)
            .await,
        Err(AuthError::ReauthenticationRequired)
    ));
    f.db.execute(
        "UPDATE auth_sessions SET reauthenticated_at=now(),revision=revision+1 WHERE id_hash=$1",
        &[&f.source.id_hash],
    )
    .await
    .unwrap();
    assert!(matches!(
        f.service.create_invite(&f.admin, &f.source).await,
        Err(AuthError::StateChanged)
    ));
    f.db.execute(
        "DELETE FROM auth_sessions WHERE id_hash=$1",
        &[&f.source.id_hash],
    )
    .await
    .unwrap();
    assert!(matches!(
        f.service
            .revoke_user_sessions(&f.admin, &f.source, target.id.as_str())
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service
            .password_reset_link(&f.admin, &f.source, target.id.as_str())
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service
            .update_user(&f.admin, &f.source, target.id.as_str(), patch(None, None))
            .await,
        Err(AuthError::StateChanged)
    ));
    assert!(matches!(
        f.service
            .revoke_invite(&f.admin, &f.source, &invite.id)
            .await,
        Err(AuthError::StateChanged)
    ));
    assert_eq!(f.user(&target.id).await.auth_version, target.auth_version);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_changes_revoke_authority_but_noops_preserve_it() {
    let mut f = Fixture::new().await;
    let target = f.add("bob", Role::Member, true).await;
    let session = Fixture::issue(&mut f.db, &target).await;
    f.db.execute("INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,expires_at) VALUES($1::text::uuid,$2,'fixture-token-hash','fixture','read',$3,now()+interval '90 days')", &[&uuid::Uuid::new_v4().to_string(), &target.id.as_str(), &target.auth_version]).await.unwrap();
    f.db.execute("UPDATE trading_connection SET agent_token_hash='fixture-agent-hash',agent_auth_version=$2 WHERE user_id=$1", &[&target.id.as_str(), &target.auth_version]).await.unwrap();
    let link = f
        .service
        .password_reset_link(&f.admin, &f.source, target.id.as_str())
        .await
        .unwrap();
    f.service
        .update_user(
            &f.admin,
            &f.source,
            target.id.as_str(),
            patch(Some(Role::Member), Some(false)),
        )
        .await
        .unwrap();
    assert_eq!(f.user(&target.id).await.auth_version, target.auth_version);
    assert!(session_store::load(&f.db, &session.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_some());
    f.service
        .update_user(
            &f.admin,
            &f.source,
            target.id.as_str(),
            patch(Some(Role::Admin), None),
        )
        .await
        .unwrap();
    assert_eq!(
        f.user(&target.id).await.auth_version,
        target.auth_version + 1
    );
    assert!(session_store::load(&f.db, &session.raw_cookie, Utc::now())
        .await
        .unwrap()
        .is_none());
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NOT NULL FROM auth_api_tokens WHERE user_id=$1",
            &[&target.id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert!(f.db.query_one("SELECT agent_token_hash IS NULL AND agent_auth_version IS NULL FROM trading_connection WHERE user_id=$1", &[&target.id.as_str()]).await.unwrap().get::<_,bool>(0));
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NOT NULL FROM auth_links WHERE id=$1",
            &[&link.id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    let new_user = f.user(&target.id).await;
    let new_session = Fixture::issue(&mut f.db, &new_user).await;
    f.service
        .revoke_user_sessions(&f.admin, &f.source, target.id.as_str())
        .await
        .unwrap();
    assert_eq!(
        f.user(&target.id).await.auth_version,
        target.auth_version + 2
    );
    assert!(
        session_store::load(&f.db, &new_session.raw_cookie, Utc::now())
            .await
            .unwrap()
            .is_none()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_enabling_respects_enrollment_and_missing_ids() {
    let mut f = Fixture::new().await;
    for complete in [false, true] {
        let target = f
            .add(
                if complete { "bob" } else { "carol" },
                Role::Member,
                complete,
            )
            .await;
        f.service
            .update_user(
                &f.admin,
                &f.source,
                target.id.as_str(),
                patch(None, Some(true)),
            )
            .await
            .unwrap();
        let updated = f
            .service
            .update_user(
                &f.admin,
                &f.source,
                target.id.as_str(),
                patch(None, Some(false)),
            )
            .await
            .unwrap();
        assert_eq!(
            updated.status,
            if complete {
                AccountStatus::Active
            } else {
                AccountStatus::Pending
            }
        );
        assert_eq!(
            f.user(&target.id).await.totp_secret_encrypted,
            target.totp_secret_encrypted
        );
    }
    assert!(matches!(
        f.service
            .update_user(&f.admin, &f.source, "missing", patch(None, None))
            .await,
        Err(AuthError::NotFound)
    ));
    assert!(matches!(
        f.service
            .revoke_user_sessions(&f.admin, &f.source, "missing")
            .await,
        Err(AuthError::NotFound)
    ));
    assert!(matches!(
        f.service
            .password_reset_link(&f.admin, &f.source, "missing")
            .await,
        Err(AuthError::NotFound)
    ));
    assert!(matches!(
        f.service
            .revoke_invite(&f.admin, &f.source, "missing")
            .await,
        Err(AuthError::NotFound)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_competing_last_admin_removals_have_one_winner_and_self_logout() {
    let mut f = Fixture::new().await;
    let other = f.add("bob", Role::Admin, true).await;
    let _pending_admin = f.add("carol", Role::Admin, false).await;
    let other_actor = other.current(CredentialKind::BrowserSession).unwrap();
    let other_source = Fixture::issue(&mut f.db, &other).await;
    let (a, b) = tokio::join!(
        f.service.update_user(
            &f.admin,
            &f.source,
            f.admin.id.as_str(),
            patch(Some(Role::Member), None)
        ),
        f.service.update_user(
            &other_actor,
            &other_source.session,
            other.id.as_str(),
            patch(None, Some(true))
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(AuthError::LastAdministrator)) | (Err(AuthError::LastAdministrator), Ok(_))
    ));
    let winner_source = if a.is_ok() {
        &f.source
    } else {
        &other_source.session
    };
    assert!(f
        .db
        .query_opt(
            "SELECT 1 FROM auth_sessions WHERE id_hash=$1",
            &[&winner_source.id_hash]
        )
        .await
        .unwrap()
        .is_none());
    let count: i64 = f.db.query_one("SELECT count(*) FROM auth_users WHERE role='admin' AND status='active' AND totp_secret_encrypted IS NOT NULL", &[]).await.unwrap().get(0);
    assert_eq!(count, 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_invites_and_reset_links_expire_revoke_and_preserve_factor() {
    let mut f = Fixture::new().await;
    let target = f.add("bob", Role::Member, true).await;
    let issued = f.service.create_invite(&f.admin, &f.source).await.unwrap();
    let row =
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&issued.id])
            .await
            .unwrap();
    let created_at: chrono::DateTime<Utc> = row.get("created_at");
    let link = auth::link_from_row(row).unwrap();
    assert_eq!(link.expires_at - created_at, Duration::days(7));
    assert_eq!(link.kind, AccountLinkKind::Invite);
    assert!(link.user_id.is_none());
    assert!(auth::check_link(&link, None, link.expires_at).is_err());
    let serialized =
        serde_json::to_value(f.service.list_invites(&f.admin, &f.source).await.unwrap()).unwrap();
    assert!(serialized["invites"][0].get("url").is_none());
    assert!(serialized["invites"][0].get("token_hash").is_none());
    f.service
        .revoke_invite(&f.admin, &f.source, &issued.id)
        .await
        .unwrap();
    let revoked = auth::link_from_row(
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&issued.id])
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(auth::check_link(&revoked, None, Utc::now()).is_err());
    let reset = f
        .service
        .password_reset_link(&f.admin, &f.source, target.id.as_str())
        .await
        .unwrap();
    let row =
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&reset.id])
            .await
            .unwrap();
    let reset_created_at: chrono::DateTime<Utc> = row.get("created_at");
    let reset_row = auth::link_from_row(row).unwrap();
    assert_eq!(reset_row.expires_at - reset_created_at, Duration::hours(1));
    assert_eq!(reset_row.target_auth_version, Some(target.auth_version));
    let unchanged = f.user(&target.id).await;
    assert_eq!(unchanged.password_hash, target.password_hash);
    assert_eq!(
        unchanged.totp_secret_encrypted,
        target.totp_secret_encrypted
    );
    assert_eq!(unchanged.auth_version, target.auth_version);
    f.service
        .update_user(
            &f.admin,
            &f.source,
            target.id.as_str(),
            patch(None, Some(true)),
        )
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .password_reset_link(&f.admin, &f.source, target.id.as_str())
            .await,
        Err(AuthError::Forbidden)
    ));
    let pending = f.add("carol", Role::Member, false).await;
    assert!(matches!(
        f.service
            .password_reset_link(&f.admin, &f.source, pending.id.as_str())
            .await,
        Err(AuthError::Forbidden)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_demoting_or_disabling_issuer_invalidates_unconsumed_links() {
    let mut f = Fixture::new().await;
    let target = f.add("bob", Role::Member, true).await;
    for disable in [false, true] {
        let issuer = f
            .add(if disable { "dave" } else { "carol" }, Role::Admin, true)
            .await;
        let actor = issuer.current(CredentialKind::BrowserSession).unwrap();
        let source = Fixture::issue(&mut f.db, &issuer).await;
        let invite = f
            .service
            .create_invite(&actor, &source.session)
            .await
            .unwrap();
        let reset = f
            .service
            .password_reset_link(&actor, &source.session, target.id.as_str())
            .await
            .unwrap();
        f.service
            .update_user(
                &f.admin,
                &f.source,
                issuer.id.as_str(),
                if disable {
                    patch(None, Some(true))
                } else {
                    patch(Some(Role::Member), None)
                },
            )
            .await
            .unwrap();
        for id in [&invite.id, &reset.id] {
            assert!(f
                .db
                .query_one(
                    "SELECT revoked_at IS NOT NULL FROM auth_links WHERE id=$1",
                    &[id]
                )
                .await
                .unwrap()
                .get::<_, bool>(0));
        }
        assert!(matches!(
            f.service.create_invite(&actor, &source.session).await,
            Err(AuthError::StateChanged)
        ));
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_commit_failure_exposes_no_link_and_rolls_back() {
    let mut f = Fixture::new().await;
    let target = f.add("bob", Role::Member, true).await;
    f.fail_link_commit().await;
    assert!(matches!(
        f.service.create_invite(&f.admin, &f.source).await,
        Err(AuthError::Unavailable)
    ));
    assert!(matches!(
        f.service
            .password_reset_link(&f.admin, &f.source, target.id.as_str())
            .await,
        Err(AuthError::Unavailable)
    ));
    assert_eq!(
        f.db.query_one("SELECT count(*) FROM auth_links", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert!(f.service.list_users(&f.admin, &f.source).await.is_ok());
    f.db.batch_execute("CREATE FUNCTION reject_admin_account_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test commit failure'; END $$;
        CREATE CONSTRAINT TRIGGER reject_admin_account_commit AFTER UPDATE ON auth_users DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_admin_account_commit();").await.unwrap();
    assert!(matches!(
        f.service
            .update_user(
                &f.admin,
                &f.source,
                target.id.as_str(),
                patch(None, Some(true))
            )
            .await,
        Err(AuthError::Unavailable)
    ));
    assert_eq!(f.user(&target.id).await.status, AccountStatus::Active);
    assert_eq!(f.user(&target.id).await.auth_version, target.auth_version);
    f.cleanup().await;
}
