use anyhow::anyhow;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{io::Cursor, str::FromStr};

use chrono::{NaiveDate, Utc};
use csv::ReaderBuilder;

use crate::{
    database::{
        models::{
            dividend::Dividend, fx_conversion::FxConversion, interest::InterestPayment,
            trade::Trade,
        },
        queries::{
            composite::add_trade_to_db, dividend::add_dividend_to_db,
            fx_conversion::add_fx_conversion_to_db, interest::add_interest_to_db,
        },
    },
    services::{market_data::fx_rates::convert_amount, parsers::parse_timestamp},
};

#[derive(Debug, PartialEq, Eq)]
enum RecordType {
    Dividend,
    EquityTrade,
    CashInterest,
    ShareInterest,
    CashTransfer,
    FxConversion,
    Unmatched,
}

fn detect_record_type(action: &str) -> RecordType {
    if action.contains("Dividend") {
        RecordType::Dividend
    } else if action == "Withdrawal" || action == "Deposit" {
        RecordType::CashTransfer
    } else if action == "Interest on cash" {
        RecordType::CashInterest
    } else if action == "Lending interest" {
        RecordType::ShareInterest
    } else if action.contains("buy") || action.contains("sell") {
        RecordType::EquityTrade
    } else if action == "Currency conversion" {
        RecordType::FxConversion
    } else {
        RecordType::Unmatched
    }
}

fn find_column_index(
    headers: &csv::StringRecord,
    column_name: &str,
    required: bool,
) -> anyhow::Result<Option<usize>> {
    let idx = headers.iter().position(|h| h == column_name);
    if required {
        idx.ok_or_else(|| anyhow!("Missing required column: {}", column_name))
            .map(Some)
    } else {
        Ok(idx)
    }
}

fn find_column_index_any(
    headers: &csv::StringRecord,
    column_names: &[&str],
) -> anyhow::Result<usize> {
    for name in column_names {
        if let Some(idx) = headers.iter().position(|h| h == *name) {
            return Ok(idx);
        }
    }
    Err(anyhow!(
        "Missing required column: {}",
        column_names.join(" or ")
    ))
}

fn optional_field<'a>(record: &'a csv::StringRecord, idx: Option<usize>) -> Option<&'a str> {
    idx.and_then(|i| record.get(i))
        .filter(|value| !value.is_empty())
}

fn field_decimal_or_zero(record: &csv::StringRecord, idx: Option<usize>) -> Decimal {
    optional_field(record, idx)
        .and_then(|value| value.parse::<Decimal>().ok())
        .unwrap_or(dec!(0))
}

fn field_string_or(record: &csv::StringRecord, idx: Option<usize>, fallback: &str) -> String {
    optional_field(record, idx)
        .map(|value| value.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn parse_fx_from_notes(notes: &str) -> anyhow::Result<(Decimal, String, Decimal, String)> {
    let parts: Vec<&str> = notes.split("->").collect();
    if parts.len() != 2 {
        return Err(anyhow!("Unable to parse FX conversion notes: {}", notes));
    }

    let parse_side = |side: &str| -> anyhow::Result<(Decimal, String)> {
        let tokens: Vec<&str> = side.trim().split_whitespace().collect();
        if tokens.len() != 2 {
            return Err(anyhow!("Unable to parse FX conversion side: {}", side));
        }
        Ok((tokens[0].parse::<Decimal>()?, tokens[1].to_string()))
    };

    let (from_amount, from_currency) = parse_side(parts[0])?;
    let (to_amount, to_currency) = parse_side(parts[1])?;
    Ok((from_amount, from_currency, to_amount, to_currency))
}

pub async fn extract_trading212_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Trading212".to_string();

    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(cursor);
    let headers = rdr.headers()?.clone();

    let action_idx = find_column_index(&headers, "Action", true)?.unwrap();
    let amount_idx = find_column_index(&headers, "Total", true)?.unwrap();
    let share_count_idx = find_column_index(&headers, "No. of shares", true)?.unwrap();
    let price_per_share_idx = find_column_index(&headers, "Price / share", true)?.unwrap();
    let id_idx = find_column_index(&headers, "ID", true)?.unwrap();
    let isin_idx = find_column_index(&headers, "ISIN", true)?.unwrap();
    let currency_total_idx = find_column_index(&headers, "Currency (Total)", true)?.unwrap();
    let currency_price_idx =
        find_column_index(&headers, "Currency (Price / share)", true)?.unwrap();
    let timestamp_idx = find_column_index_any(&headers, &["Time", "Time (UTC)"])?;
    let fees_idx = find_column_index(&headers, "Currency conversion fee", true)?.unwrap();
    let fx_rate_idx = find_column_index(&headers, "Exchange rate", true)?.unwrap();

    let withholding_tax_idx = find_column_index(&headers, "Withholding tax", false)?;
    let withholding_tax_currency_idx =
        find_column_index(&headers, "Currency (Withholding tax)", false)?;
    let notes_idx = find_column_index(&headers, "Notes", false)?;
    let currency_conversion_from_idx = find_column_index(
        &headers,
        "Currency (Currency conversion from amount)",
        false,
    )?;
    let currency_conversion_to_idx =
        find_column_index(&headers, "Currency (Currency conversion to amount)", false)?;
    let currency_conversion_from_amount_idx =
        find_column_index(&headers, "Currency conversion from amount", false)?;
    let currency_conversion_to_amount_idx =
        find_column_index(&headers, "Currency conversion to amount", false)?;

    for result in rdr.records() {
        let record = result?;
        let action = &record[action_idx];

        let record_type = detect_record_type(action);
        match record_type {
            RecordType::Dividend => {
                let dividend = Dividend {
                    isin: record[isin_idx].to_string(),
                    date: parse_timestamp(&record[timestamp_idx])?,
                    amount: record[share_count_idx].parse::<Decimal>()?
                        * record[price_per_share_idx].parse::<Decimal>()?,
                    broker: broker.clone(),
                    currency: record[currency_price_idx].to_string(),
                    amount_eur: record[amount_idx].parse::<Decimal>()?,
                    withholding_tax: field_decimal_or_zero(&record, withholding_tax_idx),
                    withholding_tax_currency: field_string_or(
                        &record,
                        withholding_tax_currency_idx,
                        &record[currency_price_idx],
                    ),
                };
                if add_dividend_to_db(dividend.clone(), None).await? {
                    println!("💵 Dividend added: {:?}", dividend);
                }
            }
            RecordType::FxConversion => {
                let from_notes = optional_field(&record, notes_idx)
                    .and_then(|notes| parse_fx_from_notes(notes).ok());
                let from_amount = optional_field(&record, currency_conversion_from_amount_idx)
                    .and_then(|value| value.parse::<Decimal>().ok())
                    .or_else(|| from_notes.as_ref().map(|parsed| parsed.0))
                    .ok_or_else(|| anyhow!("Missing FX conversion from amount"))?;
                let from_currency = optional_field(&record, currency_conversion_from_idx)
                    .map(|value| value.to_string())
                    .or_else(|| from_notes.as_ref().map(|parsed| parsed.1.clone()))
                    .ok_or_else(|| anyhow!("Missing FX conversion from currency"))?;
                let to_amount = optional_field(&record, currency_conversion_to_amount_idx)
                    .and_then(|value| value.parse::<Decimal>().ok())
                    .or_else(|| from_notes.as_ref().map(|parsed| parsed.2))
                    .unwrap_or(dec!(0));
                let to_currency = optional_field(&record, currency_conversion_to_idx)
                    .map(|value| value.to_string())
                    .or_else(|| from_notes.as_ref().map(|parsed| parsed.3.clone()))
                    .unwrap_or_else(|| "Unknown".to_string());

                let fx_conversion = FxConversion {
                    date: parse_timestamp(&record[timestamp_idx])?,
                    broker: broker.clone(),
                    from_amount,
                    to_amount,
                    from_currency,
                    to_currency,
                    date_added: Utc::now(),
                    fees: record[fees_idx].parse::<Decimal>().unwrap_or(dec!(-0.0)) * -dec!(1.0),
                };
                add_fx_conversion_to_db(fx_conversion).await?;
            }
            RecordType::EquityTrade => {
                let trade = Trade {
                    broker: broker.clone(),
                    date: parse_timestamp(&record[timestamp_idx])?,
                    isin: record[isin_idx].to_string(),
                    avg_price_per_unit: record[price_per_share_idx].parse::<Decimal>()?,
                    eur_avg_price_per_unit: record[price_per_share_idx].parse::<Decimal>()?
                        / record[fx_rate_idx].parse::<Decimal>()?,
                    units: if record[price_per_share_idx].is_empty() {
                        dec!(0.0)
                    } else {
                        record[share_count_idx].parse::<Decimal>()?
                    },
                    direction: if record[action_idx].contains("buy") {
                        "Buy".to_string()
                    } else {
                        "Sell".to_string()
                    },
                    security_type: "Equity".to_string(),
                    currency: record[currency_price_idx].to_string(),
                    date_added: Utc::now(),
                    fees: record[fees_idx].parse::<Decimal>().unwrap_or(dec!(0.0)),
                    withholding_tax: field_decimal_or_zero(&record, withholding_tax_idx),
                    withholding_tax_currency: field_string_or(
                        &record,
                        withholding_tax_currency_idx,
                        &record[currency_total_idx],
                    ),
                };
                add_trade_to_db(trade, Some(record[id_idx].to_string())).await?;
            }
            RecordType::CashInterest => {
                let amount = if record[currency_total_idx].to_string() == "EUR" {
                    record[amount_idx].parse::<Decimal>()?
                } else {
                    convert_amount(
                        record[amount_idx].parse::<Decimal>()?,
                        &NaiveDate::from_str(
                            parse_timestamp(&record[timestamp_idx])?
                                .date_naive()
                                .to_string()
                                .as_str(),
                        )?,
                        &record[currency_total_idx],
                        "EUR",
                    )
                    .await?
                };

                let interest_payment = InterestPayment {
                    date: parse_timestamp(&record[timestamp_idx])?,
                    amount: record[amount_idx].parse::<Decimal>()?,
                    broker: broker.clone(),
                    principal: "Cash".to_string(),
                    currency: record[currency_total_idx].to_string(),
                    amount_eur: amount,
                    withholding_tax: field_decimal_or_zero(&record, withholding_tax_idx),
                    withholding_tax_currency: field_string_or(
                        &record,
                        withholding_tax_currency_idx,
                        &record[currency_total_idx],
                    ),
                };
                if add_interest_to_db(interest_payment.clone(), None).await? {
                    println!("💵 Interest payment added: {:?}", interest_payment);
                }
            }

            RecordType::ShareInterest => {
                let amount = if record[currency_total_idx].to_string() == "EUR" {
                    record[amount_idx].parse::<Decimal>()?
                } else {
                    convert_amount(
                        record[amount_idx].parse::<Decimal>()?,
                        &NaiveDate::from_str(
                            parse_timestamp(&record[timestamp_idx])?
                                .date_naive()
                                .to_string()
                                .as_str(),
                        )?,
                        &record[currency_total_idx],
                        "EUR",
                    )
                    .await?
                };

                let interest_payment = InterestPayment {
                    date: parse_timestamp(&record[timestamp_idx])?,
                    amount: record[amount_idx].parse::<Decimal>()?,
                    broker: broker.clone(),
                    principal: "Shares".to_string(),
                    currency: record[currency_total_idx].to_string(),
                    amount_eur: amount,
                    withholding_tax: field_decimal_or_zero(&record, withholding_tax_idx),
                    withholding_tax_currency: field_string_or(
                        &record,
                        withholding_tax_currency_idx,
                        &record[currency_total_idx],
                    ),
                };
                if add_interest_to_db(interest_payment.clone(), None).await? {
                    println!("💵 Interest payment added: {:?}", interest_payment);
                }
            }
            RecordType::CashTransfer => continue,
            RecordType::Unmatched => continue,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fx_notes() {
        let (from_amount, from_currency, to_amount, to_currency) =
            parse_fx_from_notes("30 USD -> 27.40 EUR").unwrap();
        assert_eq!(from_amount, dec!(30));
        assert_eq!(from_currency, "USD");
        assert_eq!(to_amount, dec!(27.40));
        assert_eq!(to_currency, "EUR");
    }

    #[test]
    fn parse_fx_notes_rejects_malformed() {
        assert!(parse_fx_from_notes("30 USD").is_err());
        assert!(parse_fx_from_notes("USD -> EUR").is_err());
    }

    #[test]
    fn detect_trading212_record_types() {
        assert!(matches!(detect_record_type("Dividend (Ordinary)"), RecordType::Dividend));
        assert_eq!(detect_record_type("Interest on cash"), RecordType::CashInterest);
        assert_eq!(detect_record_type("Lending interest"), RecordType::ShareInterest);
        assert_eq!(detect_record_type("Market buy"), RecordType::EquityTrade);
        assert_eq!(detect_record_type("Limit sell"), RecordType::EquityTrade);
        assert_eq!(detect_record_type("Currency conversion"), RecordType::FxConversion);
        assert_eq!(detect_record_type("Deposit"), RecordType::CashTransfer);
        assert_eq!(detect_record_type("Withdrawal"), RecordType::CashTransfer);
        assert_eq!(detect_record_type("Something else"), RecordType::Unmatched);
    }
}
