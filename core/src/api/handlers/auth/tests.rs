use std::{ffi::OsString, net::SocketAddr};

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use chrono::Utc;
use serde_json::{json, Value};
use tokio_postgres::Client;
use tower::ServiceExt;

use crate::{
    api::routes::create_router,
    database::{models::auth::UserId, queries::auth},
    services::auth::{
        digest, random_credential,
        session_store::{create_anonymous, issue_successor_locked, SessionGrant},
        tests::{cleanup, test_database_at},
        AuthConfig, AuthService,
    },
};

struct Environment {
    url: Option<OsString>,
    mode: Option<OsString>,
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in [("POSTGRES_URL", &self.url), ("RUST_ENV", &self.mode)] {
            if let Some(value) = value {
                std::env::set_var(key, value)
            } else {
                std::env::remove_var(key)
            }
        }
    }
}

struct Fixture {
    db: Client,
    schema: String,
    app: Router,
    alice: UserId,
    bob: UserId,
    cookie: String,
    csrf: String,
    environment: Environment,
}
impl Fixture {
    async fn new() -> Self {
        let (mut db, schema) = test_database_at(refinery::Target::Latest).await;
        let mut url = reqwest::Url::parse(
            &std::env::var("TEST_POSTGRES_URL").expect("TEST_POSTGRES_URL is mandatory"),
        )
        .unwrap();
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let query = url.query().unwrap().replace('+', "%20");
        url.set_query(Some(&query));
        let environment = Environment {
            url: std::env::var_os("POSTGRES_URL"),
            mode: std::env::var_os("RUST_ENV"),
        };
        std::env::set_var("POSTGRES_URL", url.as_str());
        std::env::set_var("RUST_ENV", "production");
        let service = AuthService::new(
            AuthConfig::parse("https://example.test", &"17".repeat(32), false).unwrap(),
        );
        let alice = auth::legacy_owner(&db).await.unwrap();
        let bob = UserId::new();
        let phc = service
            .passwords
            .hash("router fixture password".into())
            .await
            .unwrap();
        let secret = service
            .encrypt_totp(&alice, false, b"12345678901234567890")
            .unwrap();
        db.execute("UPDATE auth_users SET username='alice',password_hash=$2,totp_secret_encrypted=$3,status='active' WHERE id=$1",&[&alice.as_str(),&phc,&secret]).await.unwrap();
        let secret = service
            .encrypt_totp(&bob, false, b"abcdefghijklmnopqrst")
            .unwrap();
        db.execute("INSERT INTO auth_users(id,username,password_hash,totp_secret_encrypted,role,status) VALUES($1,'bob',$2,$3,'member','active')",&[&bob.as_str(),&phc,&secret]).await.unwrap();
        let tx = db.transaction().await.unwrap();
        auth::initialize_private_defaults(&bob, &tx).await.unwrap();
        tx.commit().await.unwrap();
        for (owner, name, units) in [
            (&alice, "Alice holding", 3_i32),
            (&bob, "Bob holding", 7_i32),
        ] {
            db.execute("INSERT INTO trade(user_id,hash,broker,date,units,avg_price_per_unit,eur_avg_price_per_unit,security_type,direction,currency,isin,date_added,withholding_tax,withholding_tax_currency) VALUES($1,'same-buy','Fixture','2020-01-01Z',$2::int,10,10,'Equity','Buy','EUR','test-isin',now(),0,'EUR')",&[&owner.as_str(),&units]).await.unwrap();
            db.execute("INSERT INTO instrument(user_id,id,last_price_update,price,name) VALUES($1,'test-isin',now(),10,$2)",&[&owner.as_str(),&name]).await.unwrap();
        }
        let user = auth::by_id(&alice, &db).await.unwrap().unwrap();
        let now = Utc::now();
        let tx = db.transaction().await.unwrap();
        let anonymous = create_anonymous(&tx, now).await.unwrap();
        let issued =
            issue_successor_locked(&tx, &anonymous.session, SessionGrant::full(&user, now), now)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        let cookie = format!("doughbox_session={}", issued.raw_cookie);
        let csrf = issued.session.csrf_token;
        let app = create_router(service).unwrap();
        Self {
            db,
            schema,
            app,
            alice,
            bob,
            cookie,
            csrf,
            environment,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: impl Into<Body>,
        headers: &[(&str, &str)],
    ) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut request = request.body(body.into()).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:23456".parse::<SocketAddr>().unwrap(),
        ));
        let response = self.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        response
    }
    async fn token(&self, scope: &str) -> String {
        let token = random_credential("doughbox_api_").unwrap();
        let hash = digest("doughbox-auth-api-v1", &token);
        self.db.execute("INSERT INTO auth_api_tokens(id,user_id,token_hash,name,scope,auth_version,created_at,expires_at) VALUES(gen_random_uuid(),$1,$2,'fixture',$3,1,now(),now()+INTERVAL '90 days')",&[&self.alice.as_str(),&hash,&scope]).await.unwrap();
        token
    }
    async fn finish(self) {
        let Self {
            db,
            schema,
            environment,
            ..
        } = self;
        cleanup(db, &schema).await;
        drop(environment);
    }
}
async fn data(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn pricing_database_router_error_details_require_an_admin_browser_session() {
    use crate::{
        database::queries::{background_job, price_scheduler},
        services::market_data::prices::config::Feed,
    };

    let mut f = Fixture::new().await;
    price_scheduler::initialize(&mut f.db).await.unwrap();
    background_job::enqueue(&mut f.db, &[Feed::Xetra]).await.unwrap();
    let job = background_job::claim(&f.db, &[Feed::Xetra]).await.unwrap().unwrap();
    let job_error = "Xetra download failed: provider returned HTTP 503\nCaused by: upstream unavailable";
    background_job::fail(&mut f.db, &job, job_error, 60).await.unwrap();
    let schedule_error = "Tradegate refresh failed: invalid CSV timestamp";
    f.db.execute(
        "UPDATE price_feed_schedule SET last_error = $2 WHERE feed = $1",
        &[&Feed::Tradegate.key(), &schedule_error],
    ).await.unwrap();

    let bob = auth::by_id(&f.bob, &f.db).await.unwrap().unwrap();
    let now = Utc::now();
    let tx = f.db.transaction().await.unwrap();
    let anonymous = create_anonymous(&tx, now).await.unwrap();
    let issued = issue_successor_locked(
        &tx, &anonymous.session, SessionGrant::full(&bob, now), now,
    ).await.unwrap();
    tx.commit().await.unwrap();
    let member_cookie = format!("doughbox_session={}", issued.raw_cookie);
    let read = format!("Bearer {}", f.token("read").await);
    let write = format!("Bearer {}", f.token("write").await);

    for (headers, detailed) in [
        ([("cookie", f.cookie.as_str())], true),
        ([("cookie", member_cookie.as_str())], false),
        ([("authorization", read.as_str())], false),
        ([("authorization", write.as_str())], false),
    ] {
        let response = f.request("GET", "/api/prices", Body::empty(), &headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        let status = data(response).await;
        let feeds = status["feeds"].as_array().unwrap();
        for (feed, error) in [(Feed::Xetra, job_error), (Feed::Tradegate, schedule_error)] {
            let actual = feeds.iter().find(|row| row["feed"] == feed.key()).unwrap();
            if detailed {
                assert_eq!(actual["last_error"], error);
            } else {
                let message = actual["last_error"].as_str().unwrap();
                assert!(!message.contains(error));
            }
        }
        let healthy = feeds.iter().find(|row| row["feed"] == Feed::Frankfurt.key()).unwrap();
        assert!(healthy["last_error"].is_null());
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_router_rotated_cookie_cannot_replace_live_successor() {
    let f = Fixture::new().await;
    let response = f
        .request(
            "POST",
            "/api/auth/password/change",
            json!({"new_password": "rotated router fixture password"}).to_string(),
            &[
                ("cookie", &f.cookie),
                ("origin", "https://example.test"),
                ("x-csrf-token", &f.csrf),
                ("content-type", "application/json"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let successor = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let successor_state = data(response).await;
    assert_eq!(successor_state["stage"], "authenticated");
    let response = f
        .request(
            "GET",
            "/api/auth/state",
            Body::empty(),
            &[("cookie", &f.cookie)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(data(response).await["error"], "AuthenticationStateChanged");
    let response = f
        .request(
            "GET",
            "/api/auth/state",
            Body::empty(),
            &[("cookie", &successor)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("set-cookie"));
    let current = data(response).await;
    assert_eq!(current["stage"], "authenticated");
    assert_eq!(current["user"]["id"], f.alice.as_str());
    assert!(current["csrf_token"] == successor_state["csrf_token"]);
    let response = f
        .request(
            "GET",
            "/api/positions",
            Body::empty(),
            &[("cookie", &successor)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(data(response).await[0]["name"], "Alice holding");
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_router_invalid_cookie_requires_explicit_same_origin_logout() {
    let f = Fixture::new().await;
    for invalidation in ["expired", "revoked", "malformed"] {
        let response = f
            .request("GET", "/api/auth/state", Body::empty(), &[])
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let state = data(response).await;
        assert_eq!(state["stage"], "anonymous");
        let hash = crate::services::auth::session_store::session_digest(
            cookie.split_once('=').unwrap().1,
        );
        match invalidation {
            "expired" => {
                f.db.execute(
                    "UPDATE auth_sessions SET expires_at=now()-INTERVAL '1 second' WHERE id_hash=$1",
                    &[&hash],
                )
                .await
                .unwrap();
            }
            _ => {
                f.db.execute("DELETE FROM auth_sessions WHERE id_hash=$1", &[&hash])
                    .await
                    .unwrap();
            }
        }
        let cookie = if invalidation == "malformed" {
            "doughbox_session=invalid".to_owned()
        } else {
            cookie
        };
        let sessions_before: i64 = f.db.query_one("SELECT count(*) FROM auth_sessions", &[])
            .await.unwrap().get(0);
        let response = f
            .request(
                "GET",
                "/api/auth/state",
                Body::empty(),
                &[("cookie", &cookie)],
            )
            .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!response.headers().contains_key("set-cookie"));
        assert_eq!(data(response).await["error"], "AuthenticationStateChanged");
        let sessions_after: i64 = f.db.query_one("SELECT count(*) FROM auth_sessions", &[])
            .await.unwrap().get(0);
        assert_eq!(sessions_before, sessions_after);
        for headers in [
            vec![("cookie", cookie.as_str())],
            vec![("cookie", cookie.as_str()), ("origin", "https://attacker.test")],
            vec![
                ("cookie", cookie.as_str()),
                ("origin", "https://example.test"),
                ("sec-fetch-site", "cross-site"),
            ],
        ] {
            let response = f
                .request("POST", "/api/auth/logout", Body::empty(), &headers)
                .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(!response.headers().contains_key("set-cookie"));
            assert_eq!(data(response).await["error"], "CsrfRejected");
        }
        let response = f
            .request(
                "POST",
                "/api/auth/login",
                "not-json",
                &[
                    ("cookie", &cookie),
                    ("origin", "https://example.test"),
                    ("content-type", "application/json"),
                ],
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(data(response).await["error"], "CsrfRejected");
        let response = f
            .request(
                "POST",
                "/api/auth/logout",
                Body::empty(),
                &[("cookie", &cookie), ("origin", "https://example.test")],
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cleared = response.headers()["set-cookie"].to_str().unwrap();
        assert!(cleared.starts_with("doughbox_session=;"));
        assert!(cleared.contains("Max-Age=0"));
        let response = f
            .request("GET", "/api/auth/state", Body::empty(), &[])
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("set-cookie"));
        assert_eq!(data(response).await["stage"], "anonymous");
    }
    let response = f
        .request(
            "POST",
            "/api/auth/logout",
            Body::empty(),
            &[("origin", "https://example.test")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.headers().contains_key("set-cookie"));
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_router_valid_logout_keeps_csrf_and_source_locking() {
    let f = Fixture::new().await;
    for headers in [
        vec![("cookie", f.cookie.as_str()), ("x-csrf-token", f.csrf.as_str())],
        vec![
            ("cookie", f.cookie.as_str()),
            ("origin", "https://attacker.test"),
            ("x-csrf-token", f.csrf.as_str()),
        ],
        vec![("cookie", f.cookie.as_str()), ("origin", "https://example.test")],
        vec![
            ("cookie", f.cookie.as_str()),
            ("origin", "https://example.test"),
            ("x-csrf-token", "wrong-token"),
        ],
        vec![
            ("cookie", f.cookie.as_str()),
            ("origin", "https://example.test"),
            ("x-csrf-token", f.csrf.as_str()),
            ("sec-fetch-site", "cross-site"),
        ],
    ] {
        let response = f
            .request("POST", "/api/auth/logout", Body::empty(), &headers)
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key("set-cookie"));
        assert_eq!(data(response).await["error"], "CsrfRejected");
    }
    let response = f
        .request(
            "GET",
            "/api/auth/state",
            Body::empty(),
            &[("cookie", &f.cookie)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(data(response).await["stage"], "authenticated");
    let response = f
        .request(
            "POST",
            "/api/auth/logout",
            Body::empty(),
            &[
                ("cookie", &f.cookie),
                ("origin", "https://example.test"),
                ("x-csrf-token", &f.csrf),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.headers().contains_key("set-cookie"));
    let response = f
        .request(
            "GET",
            "/api/auth/state",
            Body::empty(),
            &[("cookie", &f.cookie)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(data(response).await["error"], "AuthenticationStateChanged");
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_router_csrf_precedes_json_empty_and_multipart_extraction() {
    let f = Fixture::new().await;
    let response = f
        .request(
            "POST",
            "/api/auth/login",
            "not-json",
            &[("cookie", &f.cookie), ("content-type", "application/json")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(data(response).await["error"], "CsrfRejected");
    for path in [
        "/api/auth/logout",
        "/api/statement-imports",
        "/api/trading/proposals/other/approve",
    ] {
        let response = f
            .request(
                "POST",
                path,
                "not-a-valid-body",
                &[
                    ("cookie", &f.cookie),
                    ("origin", "https://attacker.test"),
                    ("x-csrf-token", &f.csrf),
                ],
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(data(response).await["error"], "CsrfRejected");
    }
    let response = f
        .request(
            "POST",
            "/api/auth/logout",
            Body::empty(),
            &[
                ("cookie", &f.cookie),
                ("origin", "https://example.test"),
                ("x-csrf-token", &f.csrf),
                ("sec-fetch-site", "cross-site"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        f.db.query_one("SELECT count(*) FROM statement_import_job", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let body="--upload\r\nContent-Disposition: form-data; name=\"files\"; filename=\"owned.csv\"\r\n\r\ndate,isin,broker,action,direction,units,avg_price_per_unit,currency,security_type,fees,withholding_tax,withholding_tax_currency\n2020-01-02T12:00:00Z,IE00B4L5Y983,Fixture,Trade,Buy,1,10,EUR,Equity,0,0,EUR\r\n--upload--\r\n";
    let response = f
        .request(
            "POST",
            "/api/statement-imports",
            body,
            &[
                ("cookie", &f.cookie),
                ("origin", "https://example.test"),
                ("x-csrf-token", &f.csrf),
                ("content-type", "multipart/form-data; boundary=upload"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let row =
        f.db.query_one(
            "SELECT user_id,filename,contents FROM statement_import_job",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), f.alice.as_str());
    assert_eq!(row.get::<_, &str>(1), "owned.csv");
    assert!(std::str::from_utf8(row.get::<_, &[u8]>(2))
        .unwrap()
        .contains("IE00B4L5Y983"));
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn tenant_isolation_database_router_personal_scopes_and_admin_cannot_impersonate() {
    let f = Fixture::new().await;
    let read = f.token("read").await;
    let read_header = format!("Bearer {read}");
    let write = f.token("write").await;
    let write_header = format!("Bearer {write}");
    let response = f
        .request(
            "GET",
            "/api/positions",
            Body::empty(),
            &[("authorization", &read_header)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let rows = data(response).await;
    assert_eq!(rows[0]["name"], "Alice holding");
    assert_eq!(
        rows[0]["units"].as_str().unwrap().parse::<i32>().unwrap(),
        3
    );
    for (method, path) in [
        ("POST", "/api/assets"),
        ("GET", "/api/auth/api-tokens"),
        ("GET", "/api/admin/users"),
        ("GET", "/api/data-imports"),
        ("PUT", "/api/trading/connection"),
    ] {
        let response = f
            .request(
                method,
                path,
                "{}",
                &[
                    ("authorization", &read_header),
                    ("content-type", "application/json"),
                ],
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let body=json!({"name":"Token-owned cash","asset_class":"CashAccount","currency":"EUR","unit_label":"EUR","current_interest_rate_percent":"0"}).to_string();
    let response = f
        .request(
            "POST",
            "/api/assets",
            body,
            &[
                ("authorization", &write_header),
                ("content-type", "application/json"),
            ],
        )
        .await;
    assert!(response.status().is_success());
    let own = data(response).await;
    let own_id = own["id"].as_str().unwrap();
    assert_eq!(
        f.db.query_one("SELECT user_id FROM asset WHERE id=$1", &[&own_id])
            .await
            .unwrap()
            .get::<_, &str>(0),
        f.alice.as_str()
    );
    let other = uuid::Uuid::new_v4().to_string();
    f.db.execute("INSERT INTO asset(user_id,id,name,asset_class,currency,unit_label,current_interest_rate_percent) VALUES($1,$2,'Bob private cash','CashAccount','EUR','EUR',0)",&[&f.bob.as_str(),&other]).await.unwrap();
    let response = f
        .request(
            "GET",
            &format!("/api/assets/{other}"),
            Body::empty(),
            &[("cookie", &f.cookie)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = f
        .request(
            "GET",
            "/api/settings",
            Body::empty(),
            &[("cookie", &f.cookie)],
        )
        .await;
    let settings = data(response).await;
    assert!(settings.get("log_level").is_none());
    assert!(settings["telegram_chat_id"].is_null());
    let response = f
        .request(
            "GET",
            "/api/positions",
            Body::empty(),
            &[("cookie", &f.cookie), ("authorization", &read_header)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(data(response).await["error"], "AmbiguousCredentials");
    let response = f
        .request(
            "GET",
            "/api/positions",
            Body::empty(),
            &[("authorization", "Bearer doughbox_agent_unrelated")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    f.db.execute(
        "UPDATE auth_users SET auth_version=auth_version+1 WHERE id=$1",
        &[&f.alice.as_str()],
    )
    .await
    .unwrap();
    let response = f
        .request(
            "GET",
            "/api/positions",
            Body::empty(),
            &[("authorization", &read_header)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_router_store_commit_and_lookup_failures_are_structured_without_cookies() {
    let f = Fixture::new().await;
    f.db.batch_execute("CREATE FUNCTION fail_auth_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected commit failure'; END $$;CREATE CONSTRAINT TRIGGER fail_auth_commit AFTER INSERT ON auth_sessions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_auth_commit()").await.unwrap();
    let response = f
        .request("GET", "/api/auth/state", Body::empty(), &[])
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(data(response).await["error"], "AuthenticationUnavailable");
    f.db.batch_execute("DROP TRIGGER fail_auth_commit ON auth_sessions;ALTER TABLE auth_sessions RENAME TO unavailable_sessions").await.unwrap();
    let response = f
        .request(
            "GET",
            "/api/positions",
            Body::empty(),
            &[("cookie", &f.cookie)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(data(response).await["error"], "AuthenticationUnavailable");
    f.db.batch_execute("ALTER TABLE unavailable_sessions RENAME TO auth_sessions")
        .await
        .unwrap();
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated TEST_POSTGRES_URL; run the DB suite single-threaded"]
async fn auth_database_shared_symbol_cache_treats_sql_like_symbols_as_data() {
    let f = Fixture::new().await;
    let symbol = "quote'; SELECT password_hash FROM auth_users; --";
    f.db.execute(
        "INSERT INTO ticker_conversion(ticker,isin) VALUES($1,'public-isin')",
        &[&symbol],
    )
    .await
    .unwrap();
    let isin = crate::database::queries::ticker_conversion::query_isin_from_symbol(symbol)
        .await
        .unwrap();
    assert_eq!(isin, "public-isin");
    assert_eq!(
        f.db.query_one("SELECT count(*) FROM auth_users", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );
    f.finish().await;
}
