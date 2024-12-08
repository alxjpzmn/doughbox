mod api;
mod cli;
mod database;
mod services;

use api::api;
use cli::cli;
use database::run_migrations;
use services::{files::create_necessary_directories, shared::env::check_for_env_variables};

async fn run_doughbox() -> anyhow::Result<()> {
    check_for_env_variables();
    create_necessary_directories();
    run_migrations().await?;
    cli().await?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    run_doughbox().await?;
    Ok(())
}
