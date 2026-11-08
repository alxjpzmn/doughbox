use crate::services::{
    calculations::CalculationRuntime,
    data_imports::DataImportRuntime,
    market_data::prices::{config::PriceConfig, shutdown_signal, PriceRuntime},
    statement_imports::StatementImportRuntime,
    telegram_reports::TelegramReportRuntime,
};
use routes::create_router;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub mod errors;
pub mod handlers;
pub mod openapi;
pub mod routes;

pub async fn api(auth: std::sync::Arc<crate::services::auth::AuthService>) -> anyhow::Result<()> {
    let router = create_router(auth)?;
    crate::services::files::migrate_legacy_files().await?;
    crate::services::notifications::migrate_legacy_telegram().await?;
    let auth_maintenance = crate::services::auth::maintenance::AuthMaintenance::start();
    let addr = SocketAddr::from(([0, 0, 0, 0], 8084));
    let listener = TcpListener::bind(&addr).await?;
    let runtime = PriceRuntime::start(PriceConfig::default(), false).await?;
    let import_runtime = DataImportRuntime::start();
    let calculation_runtime = CalculationRuntime::start();
    let statement_runtime = StatementImportRuntime::start();
    let telegram_runtime = TelegramReportRuntime::start();
    let result = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await;
    auth_maintenance.shutdown().await;
    statement_runtime.shutdown().await;
    telegram_runtime.shutdown().await;
    import_runtime.shutdown().await;
    calculation_runtime.shutdown().await;
    runtime.shutdown().await;
    Ok(result?)
}
