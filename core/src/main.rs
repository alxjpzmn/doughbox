mod api;
mod cli;
mod database;
mod services;

use api::api;
use cli::cli;
use database::run_migrations;
use services::{
    files::create_necessary_directories,
    shared::{env::check_for_env_variables, logger::init_logger},
};

async fn run_doughbox() -> anyhow::Result<()> {
    init_logger();
    check_for_env_variables();
    create_necessary_directories();
    run_migrations().await?;
    cli().await?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("PANIC: {}", info);
        eprintln!("Hint: set RUST_BACKTRACE=1 for a full backtrace.");
    }));
    run_doughbox().await?;
    Ok(())
}
