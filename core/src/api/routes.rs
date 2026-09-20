use axum::{
    http::StatusCode,
    routing::{get, get_service, post},
    Router,
};
use tower_cookies::cookie::time::Duration;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::handlers::{
    assets::{
        archive_asset, cash_summary, create_asset, create_balance_snapshot, create_interest,
        create_trade, create_transaction, create_valuation, delete_balance_snapshot, delete_trade,
        delete_transaction, delete_valuation, get_asset, holdings, link_interest, list_assets,
        restore_asset, unlink_interest, update_asset, update_balance_snapshot, update_interest,
        update_trade, update_transaction, update_valuation,
    },
    auth::{auth_state, login, logout},
    check_auth,
    market::{brokers, instrument, instruments},
    performance::{live_performance, past_performance, performance_overview},
    portfolio::{portfolio, positions},
    taxation::{taxation, taxation_detailed, taxation_transactions},
    timeline::{net_inflow, timeline},
    trades::{buy_ins, dividends, fx_conversions, interest, trades},
};
use super::openapi::ApiDoc;

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
        .route("/assets", get(list_assets).post(create_asset))
        .route("/assets/holdings", get(holdings))
        .route("/cash-accounts/summary", get(cash_summary))
        .route(
            "/assets/{asset_id}",
            get(get_asset).put(update_asset).delete(archive_asset),
        )
        .route("/assets/{asset_id}/restore", post(restore_asset))
        .route("/assets/{asset_id}/trades", post(create_trade))
        .route(
            "/assets/{asset_id}/trades/{record_id}",
            axum::routing::put(update_trade).delete(delete_trade),
        )
        .route("/assets/{asset_id}/valuations", post(create_valuation))
        .route(
            "/assets/{asset_id}/valuations/{record_id}",
            axum::routing::put(update_valuation).delete(delete_valuation),
        )
        .route("/assets/{asset_id}/transactions", post(create_transaction))
        .route(
            "/assets/{asset_id}/transactions/{record_id}",
            axum::routing::put(update_transaction).delete(delete_transaction),
        )
        .route(
            "/assets/{asset_id}/balance-snapshots",
            post(create_balance_snapshot),
        )
        .route(
            "/assets/{asset_id}/balance-snapshots/{record_id}",
            axum::routing::put(update_balance_snapshot).delete(delete_balance_snapshot),
        )
        .route("/assets/{asset_id}/interest", post(create_interest))
        .route(
            "/assets/{asset_id}/interest/{interest_id}",
            post(link_interest)
                .put(update_interest)
                .delete(unlink_interest),
        )
        .route("/positions", get(positions))
        .route("/timeline", get(timeline))
        .route("/timeline/net-inflow", get(net_inflow))
        .route("/trades", get(trades))
        .route("/buy-ins", get(buy_ins))
        .route("/dividends", get(dividends))
        .route("/interest", get(interest))
        .route("/fx-conversions", get(fx_conversions))
        .route("/performance", get(live_performance))
        .route("/performance_overview", get(performance_overview))
        .route("/past_performance", get(past_performance))
        .route("/instruments", get(instruments))
        .route("/instruments/{isin}", get(instrument))
        .route("/brokers", get(brokers))
        .route("/taxation", get(taxation))
        .route("/taxation/detailed", get(taxation_detailed))
        .route("/taxation/transactions", get(taxation_transactions))
        .route("/auth_state", get(auth_state))
        .layer(axum::middleware::from_fn(check_auth));

    let cors_layer = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PATCH,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ]);

    let public_router = Router::new().nest("/api", public_routes);
    let protected_router = Router::new().nest("/api", protected_routes);
    let static_service =
        get_service(ServeDir::new("./dist").precompressed_gzip()).handle_error(|_| async move {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to serve static assets.",
            )
        });

    let static_router = Router::new().fallback_service(static_service);

    let app = public_router
        .merge(protected_router)
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .merge(static_router)
        .layer(session_layer)
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());

    Ok(app)
}
