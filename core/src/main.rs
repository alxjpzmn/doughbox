mod api;
mod cli;
mod database;
mod services;


async fn run_doughbox() -> anyhow::Result<()> {
    services::shared::logger::init_logger();
    cli::cli().await?;
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
