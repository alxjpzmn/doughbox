use dotenvy::{dotenv, from_filename, var};
use std::path::Path;

pub fn check_for_env_variables() {
    // the first to env variables are necessary for operation, thus the app panics if they aren't
    // present
    match get_env_variable("POSTGRES_URL") {
        Some(_) => println!("Postgres URL set ✅"),
        None => panic!("Please set a valid Postgres connection URL as POSTGRES_URL in your environment variables"),
    };
    match get_env_variable("POLYGON_TOKEN") {
        Some(_) => println!("Polygon token set ✅"),
        None => panic!("Please create a Polygon token via polygon.io, otherwise stock splits can't be retrieved."),
    };
    match get_env_variable("FRED_TOKEN") {
        Some(_) => println!("FRED token set ✅"),
        None => {
            println!(
                "FRED_TOKEN not set, will not be able to display portfolio alpha against SP500 ⚠️"
            )
        }
    };
    match get_env_variable("PASSWORD") {
        Some(_) => println!("Password set ✅"),
        None => println!("No password set. ⚠️"),
    };

    match get_env_variable("API_TOKEN") {
        Some(_) => println!("API token set ✅"),
        None => println!("No API token set, you'll only be able to use the CLI and Web UI. ⚠️"),
    };
}

pub fn get_env_variable(variable_to_get: &str) -> Option<String> {
    let environment = var("RUST_ENV").unwrap_or_else(|_| "development".into());

    match environment.as_str() {
        "development" => from_filename(".env.dev").ok(),
        "production" => from_filename(".env.prod").ok(),
        _ => dotenv().ok(),
    };
    var(variable_to_get).ok()
}

pub fn is_running_in_docker() -> bool {
    Path::new("/.dockerenv").exists()
}
