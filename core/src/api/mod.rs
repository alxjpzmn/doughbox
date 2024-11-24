use routes::create_router;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub mod handlers;
pub mod routes;

pub async fn api() -> anyhow::Result<()> {
    let router = create_router()?;
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();
    let addr = SocketAddr::from(([0, 0, 0, 0], 8084));
    let listener = TcpListener::bind(&addr).await?;
    Ok(axum::serve(listener, router.into_make_service()).await?)
}
