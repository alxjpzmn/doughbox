use tokio_postgres::{Client, NoTls};

pub mod models;
pub mod queries;
use refinery::embed_migrations;

use crate::services::shared::env::get_env_variable;

pub async fn db_client() -> anyhow::Result<Client> {
    let (client, connection) =
        tokio_postgres::connect(get_env_variable("POSTGRES_URL").unwrap().as_str(), NoTls).await?;

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
