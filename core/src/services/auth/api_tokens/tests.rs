use std::{ffi::OsString, net::SocketAddr, sync::Arc};

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, HeaderMap, Method, Request, StatusCode},
    middleware,
    routing::{get, put},
    Extension, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::TimeZone;
use serde_json::{json, Value};
use tokio_postgres::Client;
use tower::ServiceExt;
use tower_cookies::CookieManagerLayer;

use super::*;
use crate::{
    api::{handlers, routes::AppState},
    database::models::auth::{AuthUser, CredentialKind, Role},
    services::auth::{
        session_store::{
            create_anonymous, issue_successor_locked, session_from_row, IssuedSession, SessionGrant,
        },
        tests::{cleanup, second_connection, test_database_at},
        AuthConfig, COOKIE_NAME,
    },
};

struct ScopedEnv {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnv {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }

    fn database(schema: &str) -> Self {
        let mut url = reqwest::Url::parse(
            &std::env::var("TEST_POSTGRES_URL").expect("TEST_POSTGRES_URL is mandatory"),
        )
        .unwrap();
        let options = url
            .query_pairs()
            .find(|(key, _)| key == "options")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key != "options")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("options", &format!("{options} -csearch_path={schema}"));
        let query = url.query().unwrap().replace('+', "%20");
        url.set_query(Some(&query));
        Self::set("POSTGRES_URL", url.as_str())
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => std::env::set_var(self.key, previous),
            None => std::env::remove_var(self.key),
        }
    }
}

struct Fixture {
    db: Client,
    schema: String,
    service: Arc<AuthService>,
    alice: AuthUser,
    _url: ScopedEnv,
}

impl Fixture {
    async fn new() -> Self {
        let (db, schema) = test_database_at(refinery::Target::Latest).await;
        let url = ScopedEnv::database(&schema);
        let service = AuthService::new(
            AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
        );
        let id = auth::legacy_owner(&db).await.unwrap();
        let secret = service
            .encrypt_totp(&id, false, b"12345678901234567890")
            .unwrap();
        db.execute(
            "UPDATE auth_users SET username='alice',password_hash='fixture-phc',totp_secret_encrypted=$2,status='active' WHERE id=$1",
            &[&id.as_str(), &secret],
        ).await.unwrap();
        let alice = auth::by_id(&id, &db).await.unwrap().unwrap();
        Self {
            db,
            schema,
            service,
            alice,
            _url: url,
        }
    }

    async fn member(&mut self) -> AuthUser {
        let id = UserId::new();
        let secret = self
            .service
            .encrypt_totp(&id, false, b"12345678901234567890")
            .unwrap();
        let tx = self.db.transaction().await.unwrap();
        tx.execute(
            "INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'bob','fixture-phc',$2,'member','active')",
            &[&id.as_str(), &secret],
        ).await.unwrap();
        auth::initialize_private_defaults(&id, &tx).await.unwrap();
        tx.commit().await.unwrap();
        auth::by_id(&id, &self.db).await.unwrap().unwrap()
    }

    async fn issue(&mut self, user: &AuthUser) -> IssuedSession {
        let now = Utc::now();
        let tx = self.db.transaction().await.unwrap();
        let anonymous = create_anonymous(&tx, now).await.unwrap();
        let issued =
            issue_successor_locked(&tx, &anonymous.session, SessionGrant::full(user, now), now)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        issued
    }

    async fn alice_session(&mut self) -> IssuedSession {
        let actor = actor(&self.alice);
        let user = auth::by_id(&actor.id, &self.db).await.unwrap().unwrap();
        self.issue(&user).await
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

    async fn count(&self) -> i64 {
        self.db
            .query_one("SELECT count(*) FROM auth_api_tokens", &[])
            .await
            .unwrap()
            .get(0)
    }

    fn router(&self) -> Router {
        let state = AppState {
            auth: self.service.clone(),
        };
        let routes = handlers::api_tokens::router()
            .route("/assets", get(handlers::assets::list_assets))
            .route("/assets/{asset_id}", get(handlers::assets::get_asset))
            .route("/settings/telegram", put(handlers::settings::save_telegram))
            .route(
                "/admin/settings",
                get(handlers::settings::get_admin_settings),
            )
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                handlers::check_auth,
            ));
        Router::new()
            .nest("/api", routes)
            .with_state(state)
            .layer(Extension(ConnectInfo(
                "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
            )))
            .layer(CookieManagerLayer::new())
            .layer(middleware::from_fn(handlers::response_policy))
    }

    async fn cleanup(self) {
        cleanup(self.db, &self.schema).await;
    }
}

fn actor(user: &AuthUser) -> CurrentUser {
    user.current(CredentialKind::BrowserSession).unwrap()
}

fn browser(method: Method, uri: &str, source: &IssuedSession, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!("{COOKIE_NAME}={}", source.raw_cookie),
        )
        .header(header::ORIGIN, "https://auth.example.test")
        .header("x-csrf-token", &source.session.csrf_token)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn bearer(method: Method, uri: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, body)
}

async fn rejected_mutations(
    f: &Fixture,
    actor: &CurrentUser,
    source: &AuthSession,
    id: &str,
    code: &str,
) {
    let error = f
        .service
        .create_api_token(actor, source, "rejected".into(), ApiTokenScope::Write)
        .await
        .err()
        .expect("creation must reject stale authority");
    assert_eq!(error.code(), code);
    let error = f
        .service
        .revoke_api_token(actor, source, id)
        .await
        .err()
        .expect("revocation must reject stale authority");
    assert_eq!(error.code(), code);
}

#[test]
fn personal_token_requests_and_unicode_name_policy_are_strict() {
    for scope in ["read", "write"] {
        let request: CreateApiTokenRequest =
            serde_json::from_value(json!({"name":"test", "scope":scope})).unwrap();
        assert_eq!(request.scope.as_str(), scope);
    }
    for invalid in [
        json!({"name":"test", "scope":"admin"}),
        json!({"name":"test", "scope":"READ"}),
        json!({"name":"test", "scope":"read", "user_id":"other"}),
        json!({"name":"test"}),
    ] {
        assert!(serde_json::from_value::<CreateApiTokenRequest>(invalid).is_err());
    }
    assert_eq!(
        token_name(" \t\u{2003}Reporting token\n").unwrap(),
        "Reporting token"
    );
    assert_eq!(token_name("  界  ").unwrap(), "界");
    assert!(token_name(&"界".repeat(80)).is_ok());
    for invalid in [String::new(), " \t\n\u{2003}".into(), "界".repeat(81)] {
        assert!(matches!(
            token_name(&invalid),
            Err(AuthError::InvalidInput(_))
        ));
    }
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_tokens_are_owned_hashed_once_and_expire_after_ninety_days() {
    let mut f = Fixture::new().await;
    let alice = actor(&f.alice);
    let alice_session = f.alice_session().await;
    let bob_user = f.member().await;
    let bob = actor(&bob_user);
    let bob_session = f.issue(&bob_user).await;
    let name = format!(" \u{2003}{}\n", "界".repeat(80));
    let token = f
        .service
        .create_api_token(&alice, &alice_session.session, name, ApiTokenScope::Read)
        .await
        .unwrap();
    let other = f
        .service
        .create_api_token(
            &bob,
            &bob_session.session,
            "Bob".into(),
            ApiTokenScope::Write,
        )
        .await
        .unwrap();
    assert_ne!(token.token, other.token);
    let random = token.token.strip_prefix("doughbox_api_").unwrap();
    assert_eq!(URL_SAFE_NO_PAD.decode(random).unwrap().len(), 32);
    assert!(!random.contains('='));
    let row = f.db.query_one(
        "SELECT token_hash,name,scope,auth_version,created_at,expires_at,to_jsonb(t) AS stored FROM auth_api_tokens t WHERE id::text=$1",
        &[&token.id],
    ).await.unwrap();
    let hash: &str = row.get("token_hash");
    assert_eq!(hash, digest("doughbox-auth-api-v1", &token.token));
    assert_ne!(hash, digest("doughbox-auth-session-v1", &token.token));
    assert_eq!(row.get::<_, &str>("name"), "界".repeat(80));
    assert_eq!(row.get::<_, &str>("scope"), "read");
    assert_eq!(row.get::<_, i64>("auth_version"), alice.auth_version);
    let created_at: DateTime<Utc> = row.get("created_at");
    let expires_at: DateTime<Utc> = row.get("expires_at");
    assert_eq!(expires_at - created_at, Duration::days(90));
    assert_eq!(token.expires_at, expires_at);
    assert!(!row
        .get::<_, Value>("stored")
        .to_string()
        .contains(&token.token));
    let listed = f
        .service
        .api_tokens(&alice, &alice_session.session)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, token.id);
    assert_eq!(listed[0].scope, ApiTokenScope::Read);
    assert!(!serde_json::to_string(&listed)
        .unwrap()
        .contains(&token.token));
    let missing_id = uuid::Uuid::new_v4().to_string();
    for id in [other.id.as_str(), "missing-id", missing_id.as_str()] {
        assert!(matches!(
            f.service
                .revoke_api_token(&alice, &alice_session.session, id)
                .await,
            Err(AuthError::NotFound)
        ));
    }
    assert_eq!(
        f.service
            .api_tokens(&bob, &bob_session.session)
            .await
            .unwrap()
            .len(),
        1
    );
    f.service
        .revoke_api_token(&alice, &alice_session.session, &token.id)
        .await
        .unwrap();
    assert!(f
        .service
        .api_tokens(&alice, &alice_session.session)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        f.service
            .api_tokens(&bob, &bob_session.session)
            .await
            .unwrap()[0]
            .id,
        other.id
    );
    let stored = f.stored(&alice_session.session).await;
    assert_eq!(stored.revision, alice_session.session.revision);
    assert_eq!(stored.csrf_token, alice_session.session.csrf_token);
    assert_eq!(
        auth::by_id(&alice.id, &f.db)
            .await
            .unwrap()
            .unwrap()
            .auth_version,
        alice.auth_version
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_token_listing_excludes_exact_expiry_and_revocation() {
    let mut f = Fixture::new().await;
    let alice = actor(&f.alice);
    let session = f.alice_session().await;
    let token = f
        .service
        .create_api_token(
            &alice,
            &session.session,
            "boundary".into(),
            ApiTokenScope::Read,
        )
        .await
        .unwrap();
    let expiry = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
    f.db.execute(
        "UPDATE auth_api_tokens SET expires_at=$2 WHERE id::text=$1",
        &[&token.id, &expiry],
    )
    .await
    .unwrap();
    assert_eq!(
        summaries(&alice.id, &f.db, expiry - Duration::microseconds(1))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(summaries(&alice.id, &f.db, expiry)
        .await
        .unwrap()
        .is_empty());
    assert!(
        summaries(&alice.id, &f.db, expiry + Duration::microseconds(1))
            .await
            .unwrap()
            .is_empty()
    );
    f.service
        .revoke_api_token(&alice, &session.session, &token.id)
        .await
        .unwrap();
    assert!(
        summaries(&alice.id, &f.db, expiry - Duration::microseconds(1))
            .await
            .unwrap()
            .is_empty()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_token_mutations_recheck_recent_browser_actor_and_locked_source() {
    let mut f = Fixture::new().await;
    let alice = actor(&f.alice);
    let session = f.alice_session().await;
    let token = f
        .service
        .create_api_token(
            &alice,
            &session.session,
            "unchanged".into(),
            ApiTokenScope::Write,
        )
        .await
        .unwrap();
    for credential in [
        CredentialKind::PersonalReadToken,
        CredentialKind::PersonalWriteToken,
    ] {
        let mut bearer = alice.clone();
        bearer.credential = credential;
        rejected_mutations(&f, &bearer, &session.session, &token.id, "Forbidden").await;
        assert!(matches!(
            f.service.api_tokens(&bearer, &session.session).await,
            Err(AuthError::Forbidden)
        ));
    }
    let mut changed_role = alice.clone();
    changed_role.role = Role::Member;
    rejected_mutations(
        &f,
        &changed_role,
        &session.session,
        &token.id,
        "AuthenticationStateChanged",
    )
    .await;
    let bob = f.member().await;
    let bob_session = f.issue(&bob).await;
    rejected_mutations(
        &f,
        &alice,
        &bob_session.session,
        &token.id,
        "AuthenticationStateChanged",
    )
    .await;
    // The request captured fresh proof, but its transaction must use the proof
    // actually persisted under the source-session lock.
    f.db.execute(
        "UPDATE auth_sessions SET reauthenticated_at=$2 WHERE id_hash=$1",
        &[
            &session.session.id_hash,
            &(Utc::now() - Duration::minutes(6)),
        ],
    )
    .await
    .unwrap();
    rejected_mutations(
        &f,
        &alice,
        &session.session,
        &token.id,
        "ReauthenticationRequired",
    )
    .await;
    assert_eq!(
        f.service
            .api_tokens(&alice, &session.session)
            .await
            .unwrap()
            .len(),
        1
    );
    f.db.execute(
        "UPDATE auth_sessions SET reauthenticated_at=$2 WHERE id_hash=$1",
        &[
            &session.session.id_hash,
            &(Utc::now() + Duration::minutes(1)),
        ],
    )
    .await
    .unwrap();
    rejected_mutations(
        &f,
        &alice,
        &session.session,
        &token.id,
        "ReauthenticationRequired",
    )
    .await;
    f.db.execute(
        "UPDATE auth_sessions SET reauthenticated_at=$2 WHERE id_hash=$1",
        &[&session.session.id_hash, &Utc::now()],
    )
    .await
    .unwrap();
    for status in ["disabled", "pending"] {
        f.db.execute(
            "UPDATE auth_users SET status=$2 WHERE id=$1",
            &[&alice.id.as_str(), &status],
        )
        .await
        .unwrap();
        rejected_mutations(
            &f,
            &alice,
            &session.session,
            &token.id,
            "AuthenticationStateChanged",
        )
        .await;
    }
    f.db.execute(
        "UPDATE auth_users SET status='active',auth_version=auth_version+1 WHERE id=$1",
        &[&alice.id.as_str()],
    )
    .await
    .unwrap();
    rejected_mutations(
        &f,
        &alice,
        &session.session,
        &token.id,
        "AuthenticationStateChanged",
    )
    .await;
    assert_eq!(f.count().await, 1);
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NULL FROM auth_api_tokens WHERE id::text=$1",
            &[&token.id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_token_creation_and_revocation_lose_to_source_revocation() {
    let mut f = Fixture::new().await;
    let alice = actor(&f.alice);
    let session = f.alice_session().await;
    let token = f
        .service
        .create_api_token(
            &alice,
            &session.session,
            "must survive rejected revoke".into(),
            ApiTokenScope::Read,
        )
        .await
        .unwrap();
    let mut other = second_connection(&f.schema).await;
    let tx = other.transaction().await.unwrap();
    auth::lock_actor(&tx, &alice, false).await.unwrap();
    // Revocation owns the account lock before both competing token mutations.
    // Neither can pass the captured-source check until this deletion commits.
    let (created, revoked, ()) = tokio::join!(
        f.service.create_api_token(
            &alice,
            &session.session,
            "must not issue".into(),
            ApiTokenScope::Write
        ),
        f.service
            .revoke_api_token(&alice, &session.session, &token.id),
        async {
            let source =
                super::super::session_store::lock_source(&tx, &session.session, Utc::now())
                    .await
                    .unwrap();
            super::super::session_store::delete_locked(&tx, &source)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        },
    );
    assert!(matches!(created, Err(AuthError::StateChanged)));
    assert!(matches!(revoked, Err(AuthError::StateChanged)));
    assert_eq!(f.count().await, 1);
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NULL FROM auth_api_tokens WHERE id::text=$1",
            &[&token.id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_token_http_commit_failures_reveal_no_secret_and_rollback() {
    let mut f = Fixture::new().await;
    let alice = actor(&f.alice);
    let session = f.alice_session().await;
    let token = f
        .service
        .create_api_token(
            &alice,
            &session.session,
            "retained".into(),
            ApiTokenScope::Read,
        )
        .await
        .unwrap();
    f.db.batch_execute(
        "CREATE FUNCTION reject_token_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'isolated token commit failure'; END $$;
         CREATE CONSTRAINT TRIGGER reject_token_commit
         AFTER INSERT OR UPDATE ON auth_api_tokens DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION reject_token_commit();",
    )
    .await
    .unwrap();
    let router = f.router();
    for request in [
        browser(
            Method::POST,
            "/api/auth/api-tokens",
            &session,
            json!({"name":"never disclosed", "scope":"write"}),
        ),
        browser(
            Method::DELETE,
            &format!("/api/auth/api-tokens/{}", token.id),
            &session,
            Value::Null,
        ),
    ] {
        let (status, headers, body) = response(&router, request).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(body["error"], "AuthenticationUnavailable");
        assert!(body.get("token").is_none());
        assert!(headers.get(header::SET_COOKIE).is_none());
    }
    assert_eq!(f.count().await, 1);
    assert!(f
        .db
        .query_one(
            "SELECT revoked_at IS NULL FROM auth_api_tokens WHERE id::text=$1",
            &[&token.id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    f.db.batch_execute(
        "DROP TRIGGER reject_token_commit ON auth_api_tokens; DROP FUNCTION reject_token_commit();",
    )
    .await
    .unwrap();
    let (status, _, body) = response(
        &router,
        browser(
            Method::POST,
            "/api/auth/api-tokens",
            &session,
            json!({"name":"committed", "scope":"write"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let raw = body["token"].as_str().unwrap();
    let id = body["id"].as_str().unwrap();
    let persisted =
        f.db.query_one(
            "SELECT token_hash,expires_at FROM auth_api_tokens WHERE id::text=$1",
            &[&id],
        )
        .await
        .unwrap();
    assert_eq!(
        persisted.get::<_, &str>(0),
        digest("doughbox-auth-api-v1", raw)
    );
    assert_eq!(
        body["expires_at"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap(),
        persisted.get::<_, DateTime<Utc>>(1)
    );
    let (status, _, list) = response(
        &router,
        browser(Method::GET, "/api/auth/api-tokens", &session, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!list.to_string().contains(raw));
    assert_eq!(list["tokens"].as_array().unwrap().len(), 2);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_tokens_enforce_real_private_permissions_and_original_version() {
    let mut f = Fixture::new().await;
    let alice = actor(&f.alice);
    let session = f.alice_session().await;
    let bob = f.member().await;
    for (user, id, name) in [
        (&alice.id, "alice-asset", "Alice savings"),
        (&bob.id, "bob-asset", "Bob savings"),
    ] {
        f.db.execute(
            "INSERT INTO asset(user_id,id,name,asset_class,currency,unit_label,current_interest_rate_percent) VALUES($1,$2,$3,'CashAccount','EUR','EUR',0)",
            &[&user.as_str(), &id, &name],
        ).await.unwrap();
    }
    let read = f
        .service
        .create_api_token(&alice, &session.session, "read".into(), ApiTokenScope::Read)
        .await
        .unwrap();
    let write = f
        .service
        .create_api_token(
            &alice,
            &session.session,
            "write".into(),
            ApiTokenScope::Write,
        )
        .await
        .unwrap();
    let router = f.router();
    for token in [&read, &write] {
        let (status, headers, body) = response(
            &router,
            bearer(Method::GET, "/api/assets", &token.token, Value::Null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert!(body.to_string().contains("Alice savings"));
        assert!(!body.to_string().contains("Bob savings"));
        assert_eq!(
            response(
                &router,
                bearer(
                    Method::GET,
                    "/api/assets/bob-asset",
                    &token.token,
                    Value::Null
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        for path in ["/api/auth/api-tokens", "/api/admin/settings"] {
            assert_eq!(
                response(
                    &router,
                    bearer(Method::GET, path, &token.token, Value::Null)
                )
                .await
                .0,
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(
        response(
            &router,
            bearer(
                Method::PUT,
                "/api/settings/telegram",
                &read.token,
                json!({"chat_id":"-alice-chat"}),
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        response(
            &router,
            bearer(
                Method::PUT,
                "/api/settings/telegram",
                &write.token,
                json!({"chat_id":"-alice-chat"}),
            )
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.db.query_one(
            "SELECT telegram_chat_id FROM user_notification_settings WHERE user_id=$1",
            &[&alice.id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, &str>(0),
        "-alice-chat"
    );
    assert!(f
        .db
        .query_one(
            "SELECT telegram_chat_id FROM user_notification_settings WHERE user_id=$1",
            &[&bob.id.as_str()]
        )
        .await
        .unwrap()
        .get::<_, Option<String>>(0)
        .is_none());
    let mut mixed = bearer(Method::GET, "/api/assets", &read.token, Value::Null);
    mixed.headers_mut().insert(
        header::COOKIE,
        format!("{COOKIE_NAME}={}", session.raw_cookie)
            .parse()
            .unwrap(),
    );
    assert_eq!(response(&router, mixed).await.0, StatusCode::BAD_REQUEST);
    f.service
        .revoke_api_token(&alice, &session.session, &read.id)
        .await
        .unwrap();
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &read.token, Value::Null)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.db.execute(
        "UPDATE auth_users SET status='disabled' WHERE id=$1",
        &[&alice.id.as_str()],
    )
    .await
    .unwrap();
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &write.token, Value::Null)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let secret = f.alice.totp_secret_encrypted.as_ref().unwrap();
    f.db.execute(
        "UPDATE auth_users SET status='pending',totp_secret_encrypted=NULL WHERE id=$1",
        &[&alice.id.as_str()],
    )
    .await
    .unwrap();
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &write.token, Value::Null)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.db.execute("UPDATE auth_users SET status='active',totp_secret_encrypted=$2,auth_version=auth_version+1 WHERE id=$1", &[&alice.id.as_str(), &secret]).await.unwrap();
    // Even an unrevoked token cannot be reassociated with a later account version.
    let row =
        f.db.query_one(
            "SELECT auth_version,revoked_at FROM auth_api_tokens WHERE id::text=$1",
            &[&write.id],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i64>(0), alice.auth_version);
    assert!(row.get::<_, Option<DateTime<Utc>>>(1).is_none());
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &write.token, Value::Null)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let current = auth::by_id(&alice.id, &f.db).await.unwrap().unwrap();
    let new_session = f.issue(&current).await;
    let new_token = f
        .service
        .create_api_token(
            &actor(&current),
            &new_session.session,
            "current".into(),
            ApiTokenScope::Read,
        )
        .await
        .unwrap();
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &new_token.token, Value::Null)
        )
        .await
        .0,
        StatusCode::OK
    );
    f.db.execute(
        "UPDATE auth_api_tokens SET expires_at=$2 WHERE id::text=$1",
        &[&new_token.id, &Utc::now()],
    )
    .await
    .unwrap();
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &new_token.token, Value::Null)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let legacy = format!("doughbox_api_{}", URL_SAFE_NO_PAD.encode([42_u8; 32]));
    let _legacy = ScopedEnv::set("API_TOKEN", &legacy);
    assert_eq!(
        response(
            &router,
            bearer(Method::GET, "/api/assets", &legacy, Value::Null)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_personal_token_http_rejects_invalid_names_fields_and_missing_csrf() {
    let mut f = Fixture::new().await;
    let session = f.alice_session().await;
    let router = f.router();
    for input in [
        json!({"name":" \n\t", "scope":"read"}),
        json!({"name":"界".repeat(81), "scope":"write"}),
        json!({"name":"test", "scope":"owner"}),
        json!({"name":"test", "scope":"read", "auth_version":2}),
    ] {
        let (status, headers, body) = response(
            &router,
            browser(Method::POST, "/api/auth/api-tokens", &session, input),
        )
        .await;
        assert!(status.is_client_error());
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert!(body.get("token").is_none());
    }
    let mut request = browser(
        Method::POST,
        "/api/auth/api-tokens",
        &session,
        json!({"name":"test", "scope":"write"}),
    );
    request.headers_mut().remove("x-csrf-token");
    assert_eq!(response(&router, request).await.0, StatusCode::FORBIDDEN);
    let mut request = browser(
        Method::POST,
        "/api/auth/api-tokens",
        &session,
        json!({"name":"test", "scope":"write"}),
    );
    request.headers_mut().insert(
        header::ORIGIN,
        "https://other.example.test".parse().unwrap(),
    );
    assert_eq!(response(&router, request).await.0, StatusCode::FORBIDDEN);
    assert_eq!(f.count().await, 0);
    f.cleanup().await;
}
