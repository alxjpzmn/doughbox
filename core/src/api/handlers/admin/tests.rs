use axum::{
    body::{to_bytes, Body},
    http::{header, Request},
    response::Response,
};
use tower::ServiceExt;
use tower_cookies::CookieManagerLayer;

use super::*;
use crate::{
    database::models::auth::{CredentialKind, Role},
    services::auth::{admin::tests::Fixture, AuthConfig, AuthService},
};

fn request(method: &str, path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn app(f: &Fixture) -> Router {
    router()
        .layer(Extension(f.admin.clone()))
        .layer(Extension(f.source.clone()))
        .layer(CookieManagerLayer::new())
        .with_state(AppState {
            auth: f.service.clone(),
        })
}

async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn administrator_http_rejects_unknown_fields_and_oversized_bodies() {
    // Extraction rejects the body before the service can open a connection.
    let service = AuthService::new(
        AuthConfig::parse("https://auth.example.test", &"17".repeat(32), false).unwrap(),
    );
    let now = chrono::Utc::now();
    let actor = CurrentUser {
        id: crate::database::models::auth::UserId::new(),
        username: "alice".into(),
        role: Role::Admin,
        auth_version: 1,
        credential: CredentialKind::BrowserSession,
    };
    let source = AuthSession {
        id_hash: "fixture-source".into(),
        stage: crate::database::models::auth::SessionStage::Authenticated,
        revision: 0,
        user_id: Some(actor.id.clone()),
        auth_version: Some(1),
        csrf_token: "fixture-csrf".into(),
        last_seen_at: now,
        expires_at: now + chrono::Duration::minutes(30),
        absolute_expires_at: now + chrono::Duration::hours(12),
        reauthenticated_at: Some(now),
        failed_factor_attempts: 0,
        pending_totp: None,
        link_id: None,
        link_bound_until: None,
    };
    let app = router()
        .layer(Extension(actor))
        .layer(Extension(source))
        .layer(CookieManagerLayer::new())
        .with_state(AppState { auth: service });
    for (method, path, body) in [
        (
            "PATCH",
            "/admin/users/other",
            "{\"username\":\"new\"}".to_owned(),
        ),
        ("POST", "/admin/invites", "{\"role\":\"admin\"}".to_owned()),
        (
            "POST",
            "/admin/users/other/revoke-sessions",
            "{\"user_id\":\"other\"}".to_owned(),
        ),
        (
            "POST",
            "/admin/users/other/password-reset-link",
            "{\"factor_reset\":true}".to_owned(),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, path, &body))
            .await
            .unwrap();
        assert_eq!(json(response).await["error"], "InvalidRequest");
    }
    let body = format!("{{\"extra\":\"{}\"}}", "x".repeat(17 * 1024));
    let response = app
        .oneshot(request("POST", "/admin/invites", &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json(response).await["error"], "InvalidRequest");
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_http_commit_failure_returns_no_url_success_or_cookie() {
    let f = Fixture::new().await;
    f.fail_link_commit().await;
    for path in [
        "/admin/invites".to_owned(),
        format!("/admin/users/{}/password-reset-link", f.admin.id.as_str()),
    ] {
        let response = app(&f).oneshot(request("POST", &path, "{}")).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let body = json(response).await;
        assert_eq!(body["error"], "AuthenticationUnavailable");
        assert!(body.get("url").is_none());
        assert!(body.get("token").is_none());
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; run with --test-threads=1"]
async fn auth_database_admin_http_self_revocation_expires_cookie_after_commit() {
    let f = Fixture::new().await;
    let path = format!("/admin/users/{}/revoke-sessions", f.admin.id.as_str());
    let response = app(&f).oneshot(request("POST", &path, "{}")).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("doughbox_session="));
    assert!(cookie.contains("Max-Age=0"));
    assert!(f
        .db
        .query_opt(
            "SELECT 1 FROM auth_sessions WHERE id_hash=$1",
            &[&f.source.id_hash]
        )
        .await
        .unwrap()
        .is_none());
    let response = app(&f)
        .oneshot(request("POST", "/admin/invites", "{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    f.cleanup().await;
}
