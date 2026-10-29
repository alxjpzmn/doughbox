use num_format::{Locale, ToFormattedString};
use rust_decimal::Decimal;

pub fn hash_string(input_string: &str) -> String {
    let hash = blake3::hash(input_string.as_bytes()).to_string();
    hash
}

pub fn round_to_decimals(input: Decimal) -> Decimal {
    input.round_dp(2)
}
pub fn format_currency(amount: Decimal, round: bool) -> String {
    if round {
        let amount_rounded: i64 = amount.round().to_string().parse::<i64>().unwrap();
        format!("€ {}", amount_rounded.to_formatted_string(&Locale::de))
    } else {
        format!("€ {:.0}", amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn hash_is_deterministic_and_distinct() {
        let first = hash_string("abc");
        let second = hash_string("abc");
        let other = hash_string("abd");
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn rounds_to_two_decimal_places() {
        assert_eq!(round_to_decimals(dec!(1.234)), dec!(1.23));
        assert_eq!(round_to_decimals(dec!(1.235)), dec!(1.24));
    }
}
