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
        Ok(idx) // Optional columns return `None` if not found
    }
}

pub async fn extract_trading212_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Trading212".to_string();

    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(cursor);
    let headers = rdr.headers()?.clone();

    // Required columns
    let action_idx = find_column_index(&headers, "Action", true)?.unwrap();
    let amount_idx = find_column_index(&headers, "Total", true)?.unwrap();
    let share_count_idx = find_column_index(&headers, "No. of shares", true)?.unwrap();
    let price_per_share_idx = find_column_index(&headers, "Price / share", true)?.unwrap();
    let id_idx = find_column_index(&headers, "ID", true)?.unwrap();
    let isin_idx = find_column_index(&headers, "ISIN", true)?.unwrap();
    let currency_idx = find_column_index(&headers, "Currency (Total)", true)?.unwrap();
    let withholding_tax_idx = find_column_index(&headers, "Withholding tax", true)?.unwrap();
    let withholding_tax_currency_idx =
        find_column_index(&headers, "Currency (Withholding tax)", true)?.unwrap();
    let timestamp_idx = find_column_index(&headers, "Time", true)?.unwrap();
    let fees_idx = find_column_index(&headers, "Currency conversion fee", true)?.unwrap();
    let fx_rate_idx = find_column_index(&headers, "Exchange rate", true)?.unwrap();

    // Optional columns
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
                    currency: record[currency_idx].to_string(),
                    amount_eur: record[amount_idx].parse::<Decimal>()?,
                    withholding_tax: record
                        .get(withholding_tax_idx)
                        .map_or(dec!(0), |value| value.parse::<Decimal>().unwrap_or(dec!(0))),
                    witholding_tax_currency: record[withholding_tax_currency_idx].to_string(),
                };
                add_dividend_to_db(dividend).await?;
            }
            RecordType::FxConversion => {
                let fx_conversion = FxConversion {
                    date: parse_timestamp(&record[timestamp_idx])?,
                    broker: broker.clone(),
                    from_amount: record[currency_conversion_from_amount_idx.unwrap()]
                        .parse::<Decimal>()?,
                    to_amount: currency_conversion_to_amount_idx
                        .and_then(|idx| record.get(idx))
                        .map(|value| value.parse::<Decimal>().unwrap_or(dec!(0)))
                        .unwrap_or(dec!(0)),
                    from_currency: record[currency_conversion_from_idx.unwrap()].to_string(),
                    to_currency: currency_conversion_to_idx
                        .and_then(|idx| record.get(idx))
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "Unknown".to_string()),
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
                    no_units: if record[price_per_share_idx].is_empty() {
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
                    currency_denomination: record[currency_idx].to_string(),
                    date_added: Utc::now(),
                    // Trading 212 only charges for fx conversions
                    fees: record[fees_idx].parse::<Decimal>().unwrap_or(dec!(0.0)),
                    withholding_tax: record[withholding_tax_idx]
                        .parse::<Decimal>()
                        .unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record[withholding_tax_idx].to_string(),
                };
                add_trade_to_db(trade, Some(record[id_idx].to_string())).await?;
            }
            RecordType::CashInterest => {
                let amount = if record[currency_idx].to_string() == "EUR" {
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
                        &record[currency_idx],
                        "EUR",
                    )
                    .await?
                };

                let interest_payment = InterestPayment {
                    date: parse_timestamp(&record[timestamp_idx])?,
                    amount: record[amount_idx].parse::<Decimal>()?,
                    broker: broker.clone(),
                    principal: "Cash".to_string(),
                    currency: record[currency_idx].to_string(),
                    amount_eur: amount,
                    withholding_tax: record[withholding_tax_idx]
                        .parse::<Decimal>()
                        .unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record[withholding_tax_currency_idx].to_string(),
                };
                add_interest_to_db(interest_payment).await?;
            }

            RecordType::ShareInterest => {
                let amount = if record[currency_idx].to_string() == "EUR" {
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
                        &record[currency_idx],
                        "EUR",
                    )
                    .await?
                };

                let interest_payment = InterestPayment {
                    date: parse_timestamp(&record[timestamp_idx])?,
                    amount: record[amount_idx].parse::<Decimal>()?,
                    broker: broker.clone(),
                    principal: "Shares".to_string(),
                    currency: record[currency_idx].to_string(),
                    amount_eur: amount,
                    withholding_tax: record[withholding_tax_idx]
                        .parse::<Decimal>()
                        .unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record[withholding_tax_currency_idx].to_string(),
                };
                add_interest_to_db(interest_payment).await?;
            }
            RecordType::CashTransfer => continue,
            RecordType::Unmatched => continue,
        }
    }

    Ok(())
}
