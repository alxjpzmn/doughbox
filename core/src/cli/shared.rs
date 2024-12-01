use std::io::{self, Write};

use num_format::{Locale, ToFormattedString};
use rust_decimal::Decimal;

pub fn confirm_action(action: &str) -> bool {
    print!("Would you like to {}? (y/n): ", action);
    io::stdout().flush().expect("Failed to flush stdout");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read input");

    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

pub fn format_currency(amount: Decimal, round: bool) -> String {
    if round {
        let amount_rounded: i64 = amount.round().to_string().parse::<i64>().unwrap();
        format!("€ {}", amount_rounded.to_formatted_string(&Locale::de))
    } else {
        format!("€ {:.0}", amount)
    }
}
