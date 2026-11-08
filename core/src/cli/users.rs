use std::sync::Arc;

use clap::Subcommand;

use crate::services::auth::AuthService;

#[derive(Debug, Subcommand, PartialEq)]
pub enum UsersCommand {
    /// Issue or rotate the reserved administrator's initial setup link.
    Bootstrap,
    /// Replace both factors through a single-use link requiring fresh enrollment.
    Recover {
        #[arg(long)]
        username: String,
        /// Explicitly restore a disabled account to pending enrollment.
        #[arg(long)]
        enable: bool,
    },
}

pub async fn run(command: UsersCommand, auth: Arc<AuthService>) -> anyhow::Result<()> {
    let issued = match command {
        UsersCommand::Bootstrap => auth.bootstrap_link().await?,
        UsersCommand::Recover { username, enable } => auth.operator_recover(&username, enable).await?,
    };
    println!("{}", issued.url);
    Ok(())
}

#[cfg(test)]
mod tests;
