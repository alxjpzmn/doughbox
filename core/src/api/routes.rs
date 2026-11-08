use std::sync::Arc;

use crate::services::auth::AuthService;

use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, get_service, post, put},
    Router,
};
use tower_cookies::CookieManagerLayer;
use tower_http::{services::ServeDir, trace::TraceLayer};
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
    auth,
    check_auth, response_policy,
    data_imports::{get_data_imports, refresh_data_import, save_data_import_scheduler},
    market::{
        brokers, instrument, instruments, prices, refresh_price_feed, refresh_prices, rename,
        reset_price_feed, save_scheduler, set_price, unpin_price,
    },
    performance::{get_performance_status, live_performance, past_performance, performance_overview, refresh_performance, save_performance_scheduler},
    portfolio::{portfolio, positions},
    settings::{get_settings, get_admin_settings, save_settings, save_telegram, save_telegram_scheduler},
    statement_imports::{get_statement_imports, upload_statement_imports, MAX_BODY_BYTES},
    taxation::{get_taxation_status, refresh_taxation, save_taxation_scheduler, taxation, taxation_detailed, taxation_transactions},
    timeline::{investment_activity, timeline},
    trades::{buy_ins, dividends, fx_conversions, interest, trades},
};
use super::openapi::ApiDoc;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
}

pub fn create_router(auth: Arc<AuthService>) -> anyhow::Result<Router> {
    let state = AppState { auth };
    let auth_routes = super::handlers::api_tokens::router()
        .route("/auth/state", get(auth::auth_state))
        .route("/auth/login", post(auth::login))
        .route("/auth/mfa/verify", post(auth::mfa_verify))
        .route("/auth/totp/start", post(auth::totp_start))
        .route("/auth/totp/confirm", post(auth::totp_confirm))
        .route("/auth/totp/pending", axum::routing::delete(auth::totp_cancel))
        .route("/auth/reauth", post(auth::reauth))
        .route("/auth/password/change", post(auth::change_password))
        .route("/auth/recovery-codes/regenerate", post(auth::regenerate_recovery_codes))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/sessions", get(auth::sessions))
        .route("/auth/sessions/{id}", axum::routing::delete(auth::revoke_session))
        .route("/auth/links/inspect", post(auth::inspect_link))
        .route("/auth/register", post(auth::register))
        .route("/auth/password/reset", post(auth::reset_password))
        .route("/auth/recover", post(auth::recover_account))
        .layer(DefaultBodyLimit::max(16 * 1024));

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
        .route("/timeline/investment-activity", get(investment_activity))
        .route(
            "/statement-imports",
            get(get_statement_imports)
                .post(upload_statement_imports)
                .layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route("/trades", get(trades))
        .route("/buy-ins", get(buy_ins))
        .route("/dividends", get(dividends))
        .route("/interest", get(interest))
        .route("/fx-conversions", get(fx_conversions))
        .route("/performance", get(live_performance))
        .route("/performance/status", get(get_performance_status))
        .route("/performance/refresh", post(refresh_performance))
        .route("/performance/scheduler", put(save_performance_scheduler))
        .route("/performance_overview", get(performance_overview))
        .route("/past_performance", get(past_performance))
        .route("/instruments", get(instruments))
        .route("/instruments/{isin}", get(instrument).patch(rename))
        .route(
            "/instruments/{isin}/price",
            put(set_price).delete(unpin_price),
        )
        .route("/prices", get(prices))
        .route("/prices/scheduler", put(save_scheduler))
        .route("/prices/refresh", post(refresh_prices))
        .route("/prices/feeds/{feed}/refresh", post(refresh_price_feed))
        .route("/prices/feeds/{feed}/reset", post(reset_price_feed))
        .route("/data-imports", get(get_data_imports))
        .route("/data-imports/{source}/refresh", post(refresh_data_import))
        .route(
            "/data-imports/{source}/scheduler",
            put(save_data_import_scheduler),
        )
        .route("/settings", get(get_settings))
        .route("/settings/telegram", put(save_telegram))
        .route("/admin/settings", get(get_admin_settings).put(save_settings).layer(DefaultBodyLimit::max(16 * 1024)))
        .route("/settings/telegram-scheduler", put(save_telegram_scheduler))
        .route("/brokers", get(brokers))
        .route("/taxation/status", get(get_taxation_status))
        .route("/taxation/refresh", post(refresh_taxation))
        .route("/taxation/scheduler", put(save_taxation_scheduler))
        .route("/taxation", get(taxation))
        .route("/taxation/detailed", get(taxation_detailed))
        .route("/taxation/transactions", get(taxation_transactions))
        .merge(super::handlers::trading::router().with_state::<AppState>(()))
        .merge(super::handlers::admin::router());

    let api_router = Router::new().nest("/api",
        auth_routes.merge(protected_routes)
            .route_layer(axum::middleware::from_fn_with_state(state.clone(), check_auth)));
    let static_service =
        get_service(ServeDir::new("./dist").precompressed_gzip()).handle_error(|_| async move {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to serve static assets.",
            )
        });

    let static_router = Router::new().fallback_service(static_service);

    let app = api_router
        .merge(crate::services::trading::mcp::router(&state.auth.config.public_origin)?.with_state::<AppState>(()))
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .route("/login", get(spa_index))
        .route("/register", get(spa_index))
        .route("/recover", get(spa_index))
        .route("/enroll", get(spa_index))
        .route("/dashboard", get(spa_index))
        .route("/dashboard/{*path}", get(spa_index))
        .merge(static_router)
        .layer(CookieManagerLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(response_policy))
        .with_state(state);

    Ok(app)
}

async fn spa_index() -> impl IntoResponse {
    match tokio::fs::read("./dist/index.html").await {
        Ok(body) => (
            [
                (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            body,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    async fn status(path: &str) -> StatusCode {
        let config = crate::services::auth::AuthConfig::parse("http://127.0.0.1:8084", &"17".repeat(32), true).unwrap();
        create_router(AuthService::new(config))
            .unwrap()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn approval_links_reach_the_spa_without_swallowing_api_routes() {
        assert_eq!(status("/api/not-a-route").await, StatusCode::NOT_FOUND);
        if !std::path::Path::new("dist/index.html").is_file() {
            return;
        }
        assert_eq!(
            status("/dashboard/trading?proposal=11111111-1111-4111-8111-111111111111").await,
            StatusCode::OK
        );
        assert_eq!(status("/login?next=%2Fdashboard%2Ftrading").await, StatusCode::OK);
    }
}

