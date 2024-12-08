use rust_decimal::Decimal;

pub fn hash_string(input_string: &str) -> String {
    let hash = blake3::hash(input_string.as_bytes()).to_string();
    hash
}

pub fn round_to_decimals(input: Decimal) -> Decimal {
    input.round_dp(2)
}
