use csv::ReaderBuilder;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{collections::HashMap, io::Cursor};

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
use chrono::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LightyearRecord {
    time: String,
    reference: String,
    _ticker: String,
    isin: String,
    action: String,
    share_count: String,
    price_per_share: String,
    total_amount: String,
    currency: String,
    // Lightyear doesn't give an fx rate for dividend entries
    _fx_rate: String,
    fee: String,
    net_amount: String,
    tax_amount: String,
}

enum RecordType {
    Dividend,
    EquityTrade,
    CashInterest,
    CashTransfer,
    FxConversion,
    Unmatched,
}

fn detect_record_type(record: &LightyearRecord) -> RecordType {
    if record.action.contains("Dividend") {
        return RecordType::Dividend;
    }
    if record.action == "Withdrawal" || record.action == "Deposit" {
        return RecordType::CashTransfer;
    }
    if record.action == "Interest" {
        return RecordType::CashInterest;
    }
    if record.action == "Conversion" {
        return RecordType::FxConversion;
    }
    if record.action.contains("Buy") || record.action.contains("Sell") {
        return RecordType::EquityTrade;
    }
    RecordType::Unmatched
}

pub async fn extract_lightyear_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Lightyear".to_string();

    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new()
        .has_headers(false)
        .from_reader(cursor.clone());
    let mut rdr2 = ReaderBuilder::new().has_headers(false).from_reader(cursor);

    let mut records_by_timestamp: HashMap<String, Vec<LightyearRecord>> = HashMap::new();

    for result in rdr.deserialize() {
        let record: LightyearRecord = result?;
        let timestamp = record.time.clone();
        records_by_timestamp
            .entry(timestamp)
            .or_default()
            .push(record);
    }

    for result in rdr2.deserialize() {
        let record: LightyearRecord = result?;

        let record_type = detect_record_type(&record);

        if &record.time == "Date" {
            continue;
        }

        let date = parse_timestamp(&record.time)?;

        match record_type {
            RecordType::FxConversion => {
                let mut fx_conversion: FxConversion;

                if record.net_amount.parse::<Decimal>()? < dec!(0.0) {
                    if let Some(matching_records) = records_by_timestamp.get(&record.time) {
                        for matching_record in matching_records {
                            if matching_record.total_amount.parse::<Decimal>()? > dec!(0.0) {
                                fx_conversion = FxConversion {
                                    date: parse_timestamp(&record.time)?,
                                    broker: broker.clone(),
                                    from_amount: record.net_amount.parse::<Decimal>()? * dec!(-1.0),
                                    to_amount: matching_record.net_amount.parse::<Decimal>()?,
                                    from_currency: record.currency.clone(),
                                    to_currency: matching_record.currency.clone(),
                                    date_added: Utc::now(),
                                    fees: record.fee.parse::<Decimal>().unwrap_or(dec!(0.0)),
                                };
                                add_fx_conversion_to_db(fx_conversion).await?;
                            }
                        }
                    };
                }
            }
            RecordType::Dividend => {
                let dividend = Dividend {
                    isin: record.isin,
                    date,
                    amount: record.total_amount.parse::<Decimal>()?,
                    broker: broker.clone(),
                    currency: record.currency.clone(),
                    amount_eur: if record.currency == "EUR" {
                        record.total_amount.parse::<Decimal>()?
                    } else {
                        convert_amount(
                            record.total_amount.parse::<Decimal>()?,
                            &date.date_naive(),
                            &record.currency,
                            "EUR",
                        )
                        .await?
                    },
                    withholding_tax: record.tax_amount.parse::<Decimal>().unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record.currency.to_string(),
                };
                add_dividend_to_db(dividend).await?;
            }
            RecordType::EquityTrade => {
                let trade = Trade {
                    broker: broker.clone(),
                    date,
                    isin: record.isin,
                    avg_price_per_unit: record.price_per_share.parse::<Decimal>()?,
                    eur_avg_price_per_unit: if record.currency == "EUR" {
                        record.price_per_share.parse::<Decimal>()?
                    } else {
                        convert_amount(
                            record.price_per_share.parse::<Decimal>()?,
                            &date.date_naive(),
                            &record.currency,
                            "EUR",
                        )
                        .await?
                    },
                    no_units: if record.price_per_share.is_empty() {
                        dec!(0.0)
                    } else {
                        record.share_count.parse::<Decimal>()?
                    },
                    direction: if record.action.contains("Buy") {
                        "Buy".to_string()
                    } else {
                        "Sell".to_string()
                    },
                    security_type: "Equity".to_string(),
                    currency_denomination: record.currency.to_string(),
                    date_added: Utc::now(),
                    fees: record.fee.parse::<Decimal>()?,
                    withholding_tax: record.tax_amount.parse::<Decimal>().unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record.currency.to_string(),
                };
                add_trade_to_db(trade, Some(record.reference)).await?;
            }
            RecordType::CashInterest => {
                let interest_payment = InterestPayment {
                    date,
                    amount: record.total_amount.parse::<Decimal>()?,
                    broker: broker.clone(),
                    principal: "Cash".to_string(),
                    currency: record.currency.clone(),
                    amount_eur: if record.currency == "EUR" {
                        record.total_amount.parse::<Decimal>()?
                    } else {
                        convert_amount(
                            record.total_amount.parse::<Decimal>()?,
                            &date.date_naive(),
                            &record.currency,
                            "EUR",
                        )
                        .await?
                    },
                    withholding_tax: record.tax_amount.parse::<Decimal>().unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record.currency.to_string(),
                };
                add_interest_to_db(interest_payment).await?;
            }
            RecordType::CashTransfer => continue,
            RecordType::Unmatched => continue,
        }
    }
    Ok(())
}
