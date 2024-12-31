use axum::{
    routing::{get, get_service, post},
    Router,
};
use reqwest::StatusCode;
use tower_cookies::cookie::time::Duration;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};

use super::handlers::{
    auth_state, check_auth, login, logout, past_performance, performance, portfolio, positions,
    taxation, timeline,
};

pub fn create_router() -> anyhow::Result<Router> {
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false)
        .with_http_only(true)
        // 90 days validity
        .with_expiry(Expiry::OnInactivity(Duration::hours(24 * 90)));

    let public_routes = Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout));

    let protected_routes = Router::new()
        .route("/portfolio", get(portfolio))
        .route("/performance_overview", get(performance))
        .route("/past_performance", get(past_performance))
        .route("/timeline", get(timeline))
        .route("/taxation", get(taxation))
        .route("/positions", get(positions))
        .route("/auth_state", get(auth_state))
        .layer(axum::middleware::from_fn(check_auth));

    let cors_layer = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let router = Router::new()
        .nest("/api", public_routes)
        .nest("/api", protected_routes)
        .layer(session_layer)
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http())
        .nest_service(
            "/",
            Router::new().fallback_service(
                get_service(ServeDir::new("./dist").precompressed_gzip()).handle_error(
                    |_| async move {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to serve static assets.",
                        )
                    },
                ),
            ),
        );
    Ok(router)
}
