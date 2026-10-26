use crate::services::market_data::prices::{config::PriceConfig, shutdown_signal, PriceRuntime};
use routes::create_router;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub mod errors;
pub mod handlers;
pub mod openapi;
pub mod routes;

pub async fn api() -> anyhow::Result<()> {
    let router = create_router()?;
    let addr = SocketAddr::from(([0, 0, 0, 0], 8084));
    let listener = TcpListener::bind(&addr).await?;
    let runtime = PriceRuntime::start(PriceConfig::default(), false).await?;
    let result = axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await;
    runtime.shutdown().await;
    Ok(result?)
}
