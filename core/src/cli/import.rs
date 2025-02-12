use fancy_regex::Regex;
use log::info;
use std::fs;
use std::io::{self, Write};
use std::process;
use walkdir::WalkDir;

use crate::services::parsers::parse_file_for_import;

pub async fn import(directory_path: &str) -> anyhow::Result<()> {
    for entry in WalkDir::new(directory_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();

        info!(target: "import", "Importing {:?}", file_path);

        match fs::read(file_path) {
            Ok(buffer) => {
                if let Err(e) = parse_file_for_import(&buffer, file_path).await {
                    eprintln!("Failed to process {}: {:?}", file_path.display(), e);
                    continue;
                }
            }
            Err(e) => {
                eprintln!("Failed to read {}: {:?}", file_path.display(), e);
                continue;
            }
        }
    }
    Ok(())
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
