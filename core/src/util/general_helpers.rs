use super::constants::{IN_DIR, OUT_DIR};
use anyhow::anyhow;
use chrono::prelude::*;
use dotenvy::{dotenv, from_filename, var};
use fancy_regex::Regex;
use num_format::{Locale, ToFormattedString};
use rust_decimal::Decimal;
use std::io::{self, Write};
use std::path::Path;
use std::{fs, process};

pub fn format_currency(amount: Decimal, round: bool) -> String {
    if round {
        let amount_rounded: i64 = amount.round().to_string().parse::<i64>().unwrap();
        format!("€ {}", amount_rounded.to_formatted_string(&Locale::de))
    } else {
        format!("€ {:.0}", amount)
    }
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

pub fn hash_string(input_string: &str) -> String {
    let hash = blake3::hash(input_string.as_bytes()).to_string();
    hash
}

pub fn return_first_match(regex_pattern: &str, text: &str) -> anyhow::Result<String> {
    let regex = Regex::new(regex_pattern)?;
    let caps = regex
        .captures(text)?
        .unwrap_or_else(|| panic!("Expected regex {} couldn't be found", regex_pattern));
    let matched_text = caps.get(0).unwrap();
    Ok(matched_text.as_str().to_string())
}

pub fn choose_match_from_regex(regex_pattern: &str, text: &str) -> anyhow::Result<String> {
    let regex = Regex::new(regex_pattern)?;

    let matches: Vec<String> = regex
        .find_iter(text)
        .filter_map(|result| result.ok().map(|mat| mat.as_str().to_string()))
        .collect();

    if matches.is_empty() {
        eprintln!("No matches found for the given regex.");
        process::exit(1);
    }

    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }

    println!("Found matches:");
    for (index, matched_text) in matches.iter().enumerate() {
        println!("{}: {}", index + 1, matched_text);
    }

    loop {
        print!("Please choose a match (1-{}): ", matches.len());
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");

        match input.trim().parse::<usize>() {
            Ok(index) if index > 0 && index <= matches.len() => {
                return Ok(matches[index - 1].clone());
            }
            _ => {
                println!(
                    "Invalid input. Please enter a number between 1 and {}.",
                    matches.len()
                );
            }
        }
    }
}

pub fn does_match_exist(regex_pattern: &str, text: &str) -> bool {
    let regex = Regex::new(regex_pattern).unwrap();
    regex.is_match(text).unwrap()
}

pub fn rem_first_and_last(value: &str) -> &str {
    let mut chars = value.chars();
    chars.next();
    chars.next_back();
    chars.as_str()
}

pub fn parse_timestamp(timestamp_str: &str) -> anyhow::Result<DateTime<Utc>> {
    let formats = [
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.3fZ",
        "%Y-%m-%dT%H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%.f%#z",
        "%d.%m.%Y %H:%M:%S",
        "%Y%m%d;%H%M%S",
        "%d/%m/%Y %H:%M:%S",
        "%d.%m.%Y %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.3f",
        "%d.%m.%Y %H:%M:%S:%f",
    ];
    for format in formats.iter() {
        if let Ok(timestamp) = NaiveDateTime::parse_from_str(timestamp_str, format) {
            return Ok(timestamp.and_utc());
        }
    }
    Err(anyhow!("Unable to parse timestamp"))
}

fn create_dir_if_nonexistent(directory_path: &str) {
    let path = Path::new(directory_path);
    if !path.exists() {
        fs::create_dir_all(path).unwrap();
        println!("Folder created at: {:?}", path);
    } else {
        println!("Folder already exists at: {:?}.", path);
    }
}

pub fn create_necessary_directories() {
    create_dir_if_nonexistent(OUT_DIR);
    create_dir_if_nonexistent(IN_DIR);
}

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

pub fn confirm_action(action: &str) -> bool {
    print!("Would you like to {}? (y/n): ", action);
    io::stdout().flush().expect("Failed to flush stdout");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read input");

    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}
