use tokio_postgres::{Client, NoTls};

use crate::services::env::get_env_variable;

pub mod models;
pub mod queries;

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
