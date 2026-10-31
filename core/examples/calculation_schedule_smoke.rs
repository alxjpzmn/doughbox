#![allow(dead_code)]
#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/cli/mod.rs"]
mod cli;
#[path = "../src/database/mod.rs"]
mod database;
#[path = "../src/services/mod.rs"]
mod services;
use api::api;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let port = std::env::var("SMOKE_API_PORT").unwrap_or_else(|_| "8084".into()).parse::<u16>()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let runtime = services::calculations::CalculationRuntime::start();
    println!("CALCULATION_SCHEDULE_SMOKE_READY");
    axum::serve(listener, api::routes::create_router()?)
        .with_graceful_shutdown(services::market_data::prices::shutdown_signal())
        .await?;
    runtime.shutdown().await;
    Ok(())
}
