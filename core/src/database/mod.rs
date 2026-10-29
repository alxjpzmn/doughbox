use tokio_postgres::{tls::NoTlsStream, Client, Connection, NoTls, Socket};

pub mod models;
pub mod queries;
use refinery::embed_migrations;

use crate::services::shared::env::get_env_variable;

/// A caller may drive the connection directly to cancel session-locked work
/// immediately if PostgreSQL drops the lock-owning session.
pub async fn db_connection() -> anyhow::Result<(Client, Connection<Socket, NoTlsStream>)> {
    let url = get_env_variable("POSTGRES_URL")
        .ok_or_else(|| anyhow::anyhow!("POSTGRES_URL is not configured"))?;
    Ok(tokio_postgres::connect(&url, NoTls).await?)
}

pub async fn db_client() -> anyhow::Result<Client> {
    let (client, connection) = db_connection().await?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {}", e);
        }
    });
    Ok(client)
}

pub async fn run_migrations() -> anyhow::Result<()> {
    embed_migrations!("migrations");
    let mut client = db_client().await?;
    let migration_report = migrations::runner().run_async(&mut client).await?;

    for migration in migration_report.applied_migrations() {
        println!(
            "Migration Applied -  Name: {}, Version: {}",
            migration.name(),
            migration.version()
        );
    }

    println!("DB migrations finished ✅");
    Ok(())
}
