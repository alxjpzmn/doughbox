use std::io::{self, Write};


pub fn confirm_action(action: &str) -> bool {
    print!("Would you like to {}? (y/N): ", action);
    io::stdout().flush().expect("Failed to flush stdout");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read input");

    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

