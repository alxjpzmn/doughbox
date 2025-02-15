use tracing::Level;

use super::env::get_env_variable;

pub fn init_logger() {
    let verbosity = get_env_variable("VERBOSITY").unwrap_or_else(|| "INFO".to_string());
    let level = match verbosity.to_uppercase().as_str() {
        "TRACE" => Level::TRACE,
        "DEBUG" => Level::DEBUG,
        "INFO" => Level::INFO,
        "WARN" => Level::WARN,
        "ERROR" => Level::ERROR,
        _ => {
            eprintln!(
                "Invalid verbosity level '{}', defaulting to INFO",
                verbosity
            );
            Level::DEBUG
        }
    };

    tracing_subscriber::fmt().with_max_level(level).init();
}

