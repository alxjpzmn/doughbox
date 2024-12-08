mod api;
mod cli;
mod database;
mod services;
mod util;

use api::api;
use cli::cli;
use services::env::check_for_env_variables;
use services::files::create_necessary_directories;
use util::migrations::run_migrations;

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
