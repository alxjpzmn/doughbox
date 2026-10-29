use fancy_regex::Regex;
use log::info;
use std::fs;
use std::io::{self, Write};
use walkdir::WalkDir;

use crate::services::parsers::{parse_file_for_import, InteractionMode};

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

pub fn choose_match_from_regex(
    regex_pattern: &str,
    text: &str,
    interaction: InteractionMode,
) -> anyhow::Result<String> {
    let regex = Regex::new(regex_pattern)?;

    let matches: Vec<String> = regex
        .find_iter(text)
        .map(|result| result.map(|mat| mat.as_str().to_string()))
        .collect::<Result<_, _>>()?;

    if matches.is_empty() {
        return Err(anyhow::anyhow!("No matches found for statement field: {}", regex_pattern));
    }

    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }

    interaction.require_interactive("Multiple possible statement field matches found")?;

    println!("Found matches:");
    for (index, matched_text) in matches.iter().enumerate() {
        println!("{}: {}", index + 1, matched_text);
    }

    loop {
        print!("Please choose a match (1-{}): ", matches.len());
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Err(anyhow::anyhow!("Input ended before a statement field was selected"));
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_selection_rejects_missing_and_ambiguous_matches() {
        assert!(choose_match_from_regex(r"\d+", "none", InteractionMode::NonInteractive).is_err());
        let error = choose_match_from_regex(r"\d+", "1 2", InteractionMode::NonInteractive)
            .unwrap_err();
        assert!(error.to_string().contains("interactive CLI"));
        assert_eq!(
            choose_match_from_regex(r"\d+", "only 123", InteractionMode::NonInteractive).unwrap(),
            "123"
        );
    }
}
