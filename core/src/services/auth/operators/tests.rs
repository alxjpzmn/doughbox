use std::{ffi::OsString, sync::Arc};

use chrono::{Duration, Utc};
use serde_json::Value;
use tokio_postgres::Client;

use crate::{
    database::{models::auth::*, queries::auth},
    services::auth::{
        digest,
        links::{issue_link_locked, IssuedAccountLink},
        session_store::{
            create_anonymous, issue_successor_locked, set_pending_locked, SessionGrant,
        },
        tests::{cleanup, test_database_at},
        AuthConfig, AuthError, AuthService,
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
            Some(previous) => std::env::set_var("POSTGRES_URL", previous),
            None => std::env::remove_var("POSTGRES_URL"),
        }
    }
}

struct Fixture {
    db: Client,
    schema: String,
    service: Arc<AuthService>,
    owner: UserId,
    _url: ScopedDatabaseUrl,
}

impl Fixture {
    async fn new() -> Self {
        let (db, schema) = test_database_at(refinery::Target::Latest).await;
        let url = ScopedDatabaseUrl::new(&schema);
        let service = AuthService::new(
            AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
        );
        let owner = auth::legacy_owner(&db).await.unwrap();
        Self {
            db,
            schema,
            service,
            owner,
            _url: url,
        }
    }

    async fn user(&self) -> AuthUser {
        auth::by_id(&self.owner, &self.db).await.unwrap().unwrap()
    }

    async fn activate(&self, disabled: bool) {
        self.db.execute(
            "UPDATE auth_users SET username='alice',password_hash='old-phc',totp_secret_encrypted='unreadable-old-key-envelope',last_totp_step=123,status=$2 WHERE id=$1",
            &[&self.owner.as_str(), &if disabled { "disabled" } else { "active" }],
        ).await.unwrap();
    }

    async fn link(
        &mut self,
        kind: AccountLinkKind,
        target: Option<&UserId>,
        issuer: Option<&UserId>,
    ) -> IssuedAccountLink {
        let target = match target {
            Some(id) => Some(auth::by_id(id, &self.db).await.unwrap().unwrap()),
            None => None,
        };
        let tx = self.db.transaction().await.unwrap();
        let now = Utc::now();
        let issued = issue_link_locked(
            &tx,
            &self.service,
            kind,
            target.as_ref(),
            issuer,
            now,
            now + Duration::hours(1),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn seed_authority(&mut self) {
        let user = self.user().await;
        let now = Utc::now();
        let tx = self.db.transaction().await.unwrap();
        for grant in [
            SessionGrant::MfaRequired {
                user_id: user.id.clone(),
                auth_version: user.auth_version,
            },
            SessionGrant::EnrollmentRequired {
                user_id: user.id.clone(),
                auth_version: user.auth_version,
            },
            SessionGrant::full(&user, now),
        ] {
            let anonymous = create_anonymous(&tx, now).await.unwrap();
            let session = issue_successor_locked(&tx, &anonymous.session, grant, now)
                .await
                .unwrap();
            set_pending_locked(
                &tx,
                &session.session,
                Some(&PendingTotp {
                    id: uuid::Uuid::new_v4().to_string(),
                    secret_encrypted: "unreadable-pending-envelope".into(),
                    expires_at: now + Duration::minutes(10),
                }),
            )
            .await
            .unwrap();
        }
        tx.execute(
            "INSERT INTO auth_recovery_codes(user_id,code_hash,used_at) VALUES($1,'unused-code',NULL),($1,'used-code',now())",
            &[&user.id.as_str()],
        ).await.unwrap();
        for scope in ["read", "write"] {
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,expires_at) VALUES($1::text::uuid,$2,$3,$3,$3,$4,now()+interval '90 days')",
                &[&id, &user.id.as_str(), &scope, &user.auth_version],
            ).await.unwrap();
        }
        tx.execute(
            "UPDATE trading_connection SET agent_token_hash='old-agent',agent_auth_version=$2 WHERE user_id=$1",
            &[&user.id.as_str(), &user.auth_version],
        ).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn seed_private_history(&self) {
        self.db
            .execute(
                "UPDATE auth_instance SET legacy_files_migrated=TRUE,legacy_telegram_migrated=TRUE",
                &[],
            )
            .await
            .unwrap();
        self.db.execute(
            "UPDATE telegram_report_schedule SET enabled=TRUE,send_time_utc='12:34',next_report_at='2027-01-01Z' WHERE user_id=$1",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "UPDATE user_notification_settings SET telegram_chat_id='private-destination' WHERE user_id=$1",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "UPDATE calculation_status SET status='succeeded',last_success_at='2026-01-01Z' WHERE user_id=$1",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "UPDATE trading_connection SET credentials='retained-broker-ciphertext',environment='demo',generation='retained-generation',account='{\"id\":\"retained-account\"}'::jsonb WHERE user_id=$1",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "INSERT INTO asset(user_id,id,name,asset_class,currency,unit_label,current_interest_rate_percent,created_at,updated_at) VALUES($1,'historical-cash','Historical savings','CashAccount','EUR','EUR',2,'2024-01-01Z','2024-01-01Z')",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "INSERT INTO asset_balance_snapshot(user_id,id,asset_id,date,kind,balance,balance_eur) VALUES($1,'opening','historical-cash','2024-01-01Z','Opening',100,100)",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "INSERT INTO interest(user_id,id,asset_id,date,amount,amount_eur,principal,currency) VALUES($1,'legacy-interest','historical-cash','2024-02-01Z',5,5,'Cash','EUR')",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "UPDATE asset SET archived_at='2025-01-01Z' WHERE user_id=$1 AND id='historical-cash'",
            &[&self.owner.as_str()],
        ).await.unwrap();
        self.db.execute(
            "INSERT INTO trading_proposals(user_id,id,generation,client_request_id,request,title,note,environment,status,trades) VALUES($1,'historical-proposal','retained-generation','historical-request','{}','Historical proposal','Retained note','demo','completed','[]')",
            &[&self.owner.as_str()],
        ).await.unwrap();
    }

    async fn private_history(&self) -> Value {
        self.db.query_one(
            "SELECT jsonb_build_object(
                'instance',(SELECT to_jsonb(i) FROM auth_instance i),
                'calculations',(SELECT jsonb_agg(to_jsonb(c) ORDER BY source) FROM calculation_status c WHERE user_id=$1),
                'schedule',(SELECT to_jsonb(s) FROM telegram_report_schedule s WHERE user_id=$1),
                'destination',(SELECT to_jsonb(n) FROM user_notification_settings n WHERE user_id=$1),
                'connection',(SELECT to_jsonb(t)-'agent_token_hash'-'agent_auth_version' FROM trading_connection t WHERE user_id=$1),
                'assets',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM asset a WHERE user_id=$1),
                'snapshots',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM asset_balance_snapshot s WHERE user_id=$1),
                'interest',(SELECT jsonb_agg(to_jsonb(i) ORDER BY id) FROM interest i WHERE user_id=$1),
                'proposals',(SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM trading_proposals p WHERE user_id=$1))",
            &[&self.owner.as_str()],
        ).await.unwrap().get(0)
    }

    async fn authentication_snapshot(&self) -> Value {
        self.db.query_one(
            "SELECT jsonb_build_object(
                'users',(SELECT jsonb_agg(to_jsonb(u) ORDER BY id) FROM auth_users u),
                'links',(SELECT jsonb_agg(to_jsonb(l) ORDER BY id) FROM auth_links l),
                'sessions',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id_hash) FROM auth_sessions s),
                'codes',(SELECT jsonb_agg(to_jsonb(c) ORDER BY code_hash) FROM auth_recovery_codes c),
                'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_api_tokens t),
                'connections',(SELECT jsonb_agg(to_jsonb(t) ORDER BY user_id) FROM trading_connection t))", &[],
        ).await.unwrap().get(0)
    }

    async fn cleanup(self) {
        cleanup(self.db, &self.schema).await;
    }
}

async fn assert_issued(
    f: &Fixture,
    issued: &IssuedAccountLink,
    kind: AccountLinkKind,
    duration: Duration,
) {
    let user = f.user().await;
    let row =
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&issued.id])
            .await
            .unwrap();
    let stored_hash: String = row.get("token_hash");
    let created_at: chrono::DateTime<Utc> = row.get("created_at");
    let link = auth::link_from_row(row).unwrap();
    assert_eq!(link.kind, kind);
    assert_eq!(link.user_id.as_ref(), Some(&user.id));
    assert_eq!(link.target_auth_version, Some(user.auth_version));
    assert_eq!(link.expires_at - created_at, duration);
    assert_eq!(issued.expires_at, link.expires_at);
    auth::check_link(&link, Some(&user), Utc::now()).unwrap();
    let url = reqwest::Url::parse(&issued.url).unwrap();
    assert_eq!(
        url.origin().ascii_serialization(),
        f.service.config.public_origin
    );
    assert_eq!(
        url.path(),
        if kind == AccountLinkKind::Bootstrap {
            "/register"
        } else {
            "/recover"
        }
    );
    let token = url.fragment().unwrap().strip_prefix("token=").unwrap();
    assert!(token.starts_with("doughbox_link_"));
    assert_eq!(stored_hash, digest("doughbox-auth-link-v1", token));
    assert_ne!(stored_hash, token);
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_concurrent_bootstrap_rotation_has_one_live_link_and_preserves_legacy_history(
) {
    let f = Fixture::new().await;
    f.seed_private_history().await;
    let before = f.private_history().await;
    let original = f.service.bootstrap_link().await.unwrap();
    let (left, right) = tokio::join!(f.service.bootstrap_link(), f.service.bootstrap_link());
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.id, right.id);
    let owner = f.user().await;
    let rows =
        f.db.query("SELECT * FROM auth_links ORDER BY id", &[])
            .await
            .unwrap();
    let valid: Vec<_> = rows
        .into_iter()
        .map(|row| auth::link_from_row(row).unwrap())
        .filter(|link| auth::check_link(link, Some(&owner), Utc::now()).is_ok())
        .collect();
    assert_eq!(valid.len(), 1);
    assert_ne!(valid[0].id, original.id);
    let live = if valid[0].id == left.id {
        &left
    } else {
        &right
    };
    assert_issued(&f, live, AccountLinkKind::Bootstrap, Duration::hours(24)).await;
    assert_eq!(before, f.private_history().await);
    assert!(auth::setup_required(&f.db).await.unwrap());
    assert!(owner.username.is_none() && owner.password_hash.is_none());
    assert_eq!(owner.role, Role::Admin);
    assert_eq!(owner.status, AccountStatus::Pending);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_bootstrap_stops_once_either_credential_is_chosen() {
    let f = Fixture::new().await;
    let initial = f.service.bootstrap_link().await.unwrap();
    for (username, password) in [
        (Some("alice"), None),
        (None, Some("chosen-phc")),
        (Some("alice"), Some("chosen-phc")),
    ] {
        f.db.execute(
            "UPDATE auth_users SET username=$2,password_hash=$3 WHERE id=$1",
            &[&f.owner.as_str(), &username, &password],
        )
        .await
        .unwrap();
        let before = f.authentication_snapshot().await;
        assert!(matches!(
            f.service.bootstrap_link().await,
            Err(AuthError::InvalidInput(_))
        ));
        assert_eq!(before, f.authentication_snapshot().await);
    }
    let row =
        f.db.query_one(
            "SELECT revoked_at FROM auth_links WHERE id=$1",
            &[&initial.id],
        )
        .await
        .unwrap();
    assert!(row.get::<_, Option<chrono::DateTime<Utc>>>(0).is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_operator_recovery_clears_both_factors_and_authority_without_decryption() {
    let mut f = Fixture::new().await;
    f.activate(false).await;
    f.seed_private_history().await;
    f.seed_authority().await;
    let owner = f.owner.clone();
    let old_reset = f
        .link(AccountLinkKind::PasswordReset, Some(&owner), Some(&owner))
        .await;
    let old_recovery = f
        .link(AccountLinkKind::OperatorRecovery, Some(&owner), None)
        .await;
    let invite = f.link(AccountLinkKind::Invite, None, Some(&owner)).await;
    let other_id = UserId::new();
    f.db.execute("INSERT INTO auth_users(id,username,password_hash,role,status,totp_secret_encrypted) VALUES($1,'bob','other-phc','member','active','other-factor')", &[&other_id.as_str()]).await.unwrap();
    let other_before =
        f.db.query_one(
            "SELECT to_jsonb(u) FROM auth_users u WHERE id=$1",
            &[&other_id.as_str()],
        )
        .await
        .unwrap()
        .get::<_, Value>(0);
    let issuer_reset = f
        .link(
            AccountLinkKind::PasswordReset,
            Some(&other_id),
            Some(&owner),
        )
        .await;
    let unrelated = f
        .link(AccountLinkKind::PasswordReset, Some(&other_id), None)
        .await;
    let user_before = f.user().await;
    assert!(matches!(
        f.service.decrypt_totp(
            &owner,
            false,
            user_before.totp_secret_encrypted.as_deref().unwrap()
        ),
        Err(AuthError::Unavailable)
    ));
    let history = f.private_history().await;
    let issued = f
        .service
        .operator_recover("  ALICE  ", false)
        .await
        .unwrap();
    assert_issued(
        &f,
        &issued,
        AccountLinkKind::OperatorRecovery,
        Duration::minutes(30),
    )
    .await;
    let after = f.user().await;
    assert_eq!(after.id, owner);
    assert_eq!(after.username.as_deref(), Some("alice"));
    assert_eq!(after.role, user_before.role);
    assert_eq!(after.created_at, user_before.created_at);
    assert_eq!(after.status, AccountStatus::Pending);
    assert_eq!(after.auth_version, user_before.auth_version + 1);
    assert!(after.password_hash.is_none());
    assert!(after.totp_secret_encrypted.is_none());
    assert!(after.last_totp_step.is_none());
    assert!(!auth::setup_required(&f.db).await.unwrap());
    let row =
        f.db.query_one(
            "SELECT (SELECT count(*) FROM auth_sessions WHERE user_id=$1),
                (SELECT count(*) FROM auth_recovery_codes WHERE user_id=$1),
                (SELECT count(*) FROM auth_api_tokens WHERE user_id=$1 AND revoked_at IS NULL)",
            &[&owner.as_str()],
        )
        .await
        .unwrap();
    assert_eq!(
        (
            row.get::<_, i64>(0),
            row.get::<_, i64>(1),
            row.get::<_, i64>(2)
        ),
        (0, 0, 0)
    );
    let row =
        f.db.query_one(
            "SELECT agent_token_hash,agent_auth_version FROM trading_connection WHERE user_id=$1",
            &[&owner.as_str()],
        )
        .await
        .unwrap();
    assert!(row.get::<_, Option<String>>(0).is_none());
    assert!(row.get::<_, Option<i64>>(1).is_none());
    for id in [
        &old_reset.id,
        &old_recovery.id,
        &invite.id,
        &issuer_reset.id,
    ] {
        let row =
            f.db.query_one("SELECT revoked_at FROM auth_links WHERE id=$1", &[&id])
                .await
                .unwrap();
        assert!(row.get::<_, Option<chrono::DateTime<Utc>>>(0).is_some());
    }
    let row =
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&unrelated.id])
            .await
            .unwrap();
    let link = auth::link_from_row(row).unwrap();
    let other = auth::by_id(&other_id, &f.db).await.unwrap().unwrap();
    auth::check_link(&link, Some(&other), Utc::now()).unwrap();
    assert_eq!(
        other_before,
        f.db.query_one(
            "SELECT to_jsonb(u) FROM auth_users u WHERE id=$1",
            &[&other_id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, Value>(0)
    );
    assert_eq!(history, f.private_history().await);
    let second = f.service.operator_recover("alice", false).await.unwrap();
    assert_issued(
        &f,
        &second,
        AccountLinkKind::OperatorRecovery,
        Duration::minutes(30),
    )
    .await;
    let first = auth::link_from_row(
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&issued.id])
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        auth::check_link(&first, Some(&f.user().await), Utc::now()),
        Err(AuthError::InvalidAccountLink)
    ));
    let second_link = auth::link_from_row(
        f.db.query_one("SELECT * FROM auth_links WHERE id=$1", &[&second.id])
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        auth::check_link(&second_link, Some(&f.user().await), second.expires_at),
        Err(AuthError::InvalidAccountLink)
    ));
    f.db.execute(
        "UPDATE auth_users SET auth_version=auth_version+1 WHERE id=$1",
        &[&owner.as_str()],
    )
    .await
    .unwrap();
    assert!(matches!(
        auth::check_link(&second_link, Some(&f.user().await), Utc::now()),
        Err(AuthError::InvalidAccountLink)
    ));
    assert_eq!(history, f.private_history().await);
    assert!(matches!(
        f.service.bootstrap_link().await,
        Err(AuthError::InvalidInput(_))
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_disabled_operator_recovery_requires_explicit_enable_and_is_atomic() {
    let mut f = Fixture::new().await;
    f.activate(true).await;
    f.seed_private_history().await;
    f.seed_authority().await;
    let owner = f.owner.clone();
    f.link(AccountLinkKind::OperatorRecovery, Some(&owner), None)
        .await;
    let before = f.authentication_snapshot().await;
    let history = f.private_history().await;
    assert!(matches!(
        f.service.operator_recover("alice", false).await,
        Err(AuthError::InvalidInput(_))
    ));
    assert!(matches!(
        f.service.operator_recover("missing-user", true).await,
        Err(AuthError::NotFound)
    ));
    assert!(matches!(
        f.service.operator_recover("not a username", true).await,
        Err(AuthError::InvalidInput(_))
    ));
    assert_eq!(before, f.authentication_snapshot().await);
    assert_eq!(history, f.private_history().await);
    let issued = f.service.operator_recover("ALICE", true).await.unwrap();
    assert_issued(
        &f,
        &issued,
        AccountLinkKind::OperatorRecovery,
        Duration::minutes(30),
    )
    .await;
    let user = f.user().await;
    assert_eq!(user.status, AccountStatus::Pending);
    assert_eq!(user.role, Role::Admin);
    assert_eq!(user.username.as_deref(), Some("alice"));
    assert!(user.password_hash.is_none() && user.totp_secret_encrypted.is_none());
    assert_eq!(history, f.private_history().await);
    f.cleanup().await;
}
