use crate::util::{
    constants::{OUT_DIR, SESSION_TOKEN_KEY},
    db_helpers::{get_all_total_active_unit_counts, get_dividends, get_performance_signals},
    general_helpers::{get_env_variable, parse_timestamp},
    taxation_helpers::get_events,
};
use axum::{
    extract::{Json, Query, Request},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, get_service, post},
    Router,
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;
use std::net::SocketAddr;
use tower_cookies::cookie::time::Duration;

use tokio::fs;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tower_sessions::{Expiry, MemoryStore, Session, SessionManagerLayer};

use super::portfolio::get_position_overview;

fn json_response<T: serde::Serialize>(
    data: &T,
) -> Result<(StatusCode, HeaderMap, String), StatusCode> {
    let data = serde_json::to_string(data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    Ok((StatusCode::OK, headers, data))
}

async fn check_auth(
    session: Session,
    request: Request,
    next: Next,
) -> anyhow::Result<Response, StatusCode> {
    if session
        .get::<String>(SESSION_TOKEN_KEY)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some()
    {
        return Ok(next.run(request).await);
    }

    if let Some(token) = get_env_variable("API_TOKEN") {
        if let Some(auth_header) = request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
        {
            if auth_header.strip_prefix("Bearer ") == Some(&token) {
                return Ok(next.run(request).await);
            }
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}

#[derive(Deserialize)]
struct LoginRequestData {
    password: String,
}

async fn issue_session_cookie(session: Session) -> anyhow::Result<(), StatusCode> {
    session
        .insert(SESSION_TOKEN_KEY, "user")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn login(
    session: Session,
    Json(payload): Json<LoginRequestData>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let password = get_env_variable("PASSWORD");
    match password {
        Some(password) if payload.password == password => {
            issue_session_cookie(session).await?;
            Ok(StatusCode::OK)
        }
        None => {
            issue_session_cookie(session).await?;
            Ok(StatusCode::OK)
        }
        _ => Ok(StatusCode::UNAUTHORIZED),
    }
}

async fn logout(session: Session) -> anyhow::Result<impl IntoResponse, StatusCode> {
    session
        .delete()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

async fn portfolio() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let position_overview = get_position_overview()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&position_overview)
}

async fn pl() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let path = format!("{}/pl.json", OUT_DIR);
    let data = fs::read_to_string(path).await.expect("Unable to read file");
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
}

async fn performance() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let performance = get_performance_signals()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&performance)
}

#[derive(Debug, Deserialize)]
struct TimelineQuery {
    start_date: String,
}

async fn timeline(query: Query<TimelineQuery>) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let year_start_timestamp = DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDate::parse_from_str(&query.start_date, "%Y-%m-%d")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        Utc,
    );

    let end_date = Utc::now();

    let timeline = get_events(year_start_timestamp, end_date)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&timeline)
}

async fn dividends() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let dividends = get_dividends()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&dividends)
}

async fn taxation() -> anyhow::Result<impl IntoResponse, StatusCode> {
    let path = format!("{}/taxation.json", OUT_DIR);
    let data = fs::read_to_string(path).await.expect("Unable to read file");
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    Ok((StatusCode::OK, headers, data))
}

#[derive(Debug, Deserialize)]
struct ActiveUnitsQuery {
    date: Option<String>,
}

async fn active_units(
    Query(query): Query<ActiveUnitsQuery>,
) -> anyhow::Result<impl IntoResponse, StatusCode> {
    let date = query.date.unwrap_or_else(|| {
        let now = Utc::now();
        format!("{}-{:02}-{:02}", now.year(), now.month(), now.day())
    });
    let timestamp = parse_timestamp(format!("{} 19:00:00", date).as_str())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let active_units = get_all_total_active_unit_counts(Some(timestamp))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    json_response(&active_units)
}

async fn auth_state() -> impl IntoResponse {
    (StatusCode::OK, "authenticated")
}

pub async fn api() -> anyhow::Result<()> {
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
        .route("/pl", get(pl))
        .route("/performance", get(performance))
        .route("/timeline", get(timeline))
        .route("/dividends", get(dividends))
        .route("/taxation", get(taxation))
        .route("/active_units", get(active_units))
        .route("/auth_state", get(auth_state))
        .layer(axum::middleware::from_fn(check_auth));

    let cors_layer = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let app = Router::new()
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

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();
    let addr = SocketAddr::from(([0, 0, 0, 0], 8084));
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    Ok(axum::serve(listener, app.into_make_service()).await?)
}
