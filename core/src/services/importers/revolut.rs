use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{collections::HashMap, io::Cursor};

use chrono::prelude::*;
use csv::{ReaderBuilder, StringRecord};
use serde::Deserialize;

use crate::{
    services::{
        instruments::{identifiers::get_changed_identifier, ticker_symbols::get_isin_from_symbol},
        parsers::parse_timestamp,
    },
    util::db_helpers::{
        add_dividend_to_db, add_fx_conversion_to_db, add_trade_to_db, get_listing_changes,
        Dividend, FxConversion, Trade,
    },
};

#[derive(Debug, Deserialize)]
pub struct RevolutTradingRecord {
    time: String,
    ticker: String,
    action: String,
    share_count: String,
    price_per_share: String,
    total_amount: String,
    currency: String,
    fx_rate: String,
}

#[derive(Debug, Deserialize)]
struct RevolutAccountRecord {
    transaction_type: String,
    _product: String,
    started_date: String,
    _completed_date: String,
    _description: String,
    amount: String,
    fee: String,
    currency: String,
    _state: String,
    _balance: String,
}

enum TradingRecordType {
    Dividend,
    EquityTrade,
    CashTransfer,
    Unmatched,
}

enum AccountRecordType {
    FxConversion,
    Unmatched,
}

enum CsvType {
    Trading,
    Account,
}

fn detect_csv_type(headers: &StringRecord) -> CsvType {
    if headers.len() == 10 {
        return CsvType::Account;
    }
    CsvType::Trading
}

fn detect_trading_record_type(record: &RevolutTradingRecord) -> TradingRecordType {
    if record.action.contains("DIVIDEND") {
        return TradingRecordType::Dividend;
    }
    if record.action == "CASH WITHDRAWAL" || record.action == "CASH TOP-UP" {
        return TradingRecordType::CashTransfer;
    }
    if record.action.contains("BUY") || record.action.contains("SELL") {
        return TradingRecordType::EquityTrade;
    }
    TradingRecordType::Unmatched
}

fn detect_account_record_type(record: &RevolutAccountRecord) -> AccountRecordType {
    if record.transaction_type == "EXCHANGE" {
        return AccountRecordType::FxConversion;
    }
    AccountRecordType::Unmatched
}

pub async fn extract_revolut_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Revolut".to_string();
    let listing_changes = get_listing_changes().await?;

    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new()
        .has_headers(false)
        .from_reader(cursor.clone());

    let csv_type = detect_csv_type(rdr.headers().unwrap());

    match csv_type {
        CsvType::Trading => {
            for result in rdr.deserialize() {
                let record: RevolutTradingRecord = result?;
                let record_type = detect_trading_record_type(&record);

                match record_type {
                    TradingRecordType::Dividend => {
                        let listing_changes = listing_changes.clone();
                        let parsed_amount = record
                            .total_amount
                            .replace("$", "")
                            .replace(",", "")
                            .parse::<Decimal>()?;

                        let dividend = Dividend {
                            isin: get_isin_from_symbol(
                                get_changed_identifier(&record.ticker, listing_changes).as_str(),
                            )
                            .await?,
                            date: Utc
                                .from_utc_datetime(&parse_timestamp(&record.time)?.naive_utc()),
                            broker: broker.clone(),
                            amount: parsed_amount,
                            currency: record.currency.clone(),
                            amount_eur: parsed_amount / record.fx_rate.parse::<Decimal>()?,
                            // Revolut takes withholding tax for dividends, but doesn't state in
                            // their export files (only in the app). Companies are US listed only,
                            // withheld dividend tax in the US 15%.
                            withholding_tax: parsed_amount * dec!(0.15),
                            witholding_tax_currency: record.currency.clone(),
                        };
                        add_dividend_to_db(dividend).await?;
                    }
                    TradingRecordType::EquityTrade => {
                        let listing_changes = listing_changes.clone();
                        let parsed_price_per_share = record
                            .price_per_share
                            .replace("$", "")
                            .replace(",", "")
                            .parse::<Decimal>()?;

                        let trade = Trade {
                            broker: broker.clone(),
                            date: Utc
                                .from_utc_datetime(&parse_timestamp(&record.time)?.naive_utc()),
                            isin: get_isin_from_symbol(
                                get_changed_identifier(&record.ticker, listing_changes).as_str(),
                            )
                            .await?,
                            avg_price_per_unit: parsed_price_per_share,
                            eur_avg_price_per_unit: parsed_price_per_share
                                / record.fx_rate.parse::<Decimal>()?,
                            no_units: if record.price_per_share.is_empty() {
                                dec!(0.0)
                            } else {
                                record.share_count.parse::<Decimal>()?
                            },
                            direction: if record.action.contains("BUY") {
                                "Buy".to_string()
                            } else {
                                "Sell".to_string()
                            },
                            security_type: "Equity".to_string(),
                            currency_denomination: record.currency.to_string(),
                            date_added: Utc::now(),
                            fees: dec!(0.0),
                            withholding_tax: dec!(0.0),
                            witholding_tax_currency: record.currency,
                        };
                        add_trade_to_db(trade, None).await?;
                    }
                    TradingRecordType::CashTransfer => continue,
                    TradingRecordType::Unmatched => continue,
                }
            }
        }
        CsvType::Account => {
            let mut rdr2 = ReaderBuilder::new().has_headers(false).from_reader(cursor);

            let mut records_by_timestamp: HashMap<String, Vec<RevolutAccountRecord>> =
                HashMap::new();

            for result in rdr2.deserialize() {
                let record: RevolutAccountRecord = result?;
                let timestamp = record.started_date.clone();
                records_by_timestamp
                    .entry(timestamp)
                    .or_default()
                    .push(record);
            }
            for result in rdr.deserialize() {
                let record: RevolutAccountRecord = result?;
                let record_type = detect_account_record_type(&record);
                match record_type {
                    AccountRecordType::FxConversion => {
                        if record.amount.parse::<Decimal>()? < dec!(0.0) {
                            if let Some(matching_records) =
                                records_by_timestamp.get(&record.started_date)
                            {
                                for matching_record in matching_records {
                                    if matching_record.amount.parse::<Decimal>()? > dec!(0.0) {
                                        let fx_conversion = FxConversion {
                                            date: parse_timestamp(&record.started_date)?,
                                            broker: broker.clone(),
                                            from_amount: record.amount.parse::<Decimal>()?
                                                * dec!(-1.0),
                                            to_amount: matching_record.amount.parse::<Decimal>()?,
                                            from_currency: record.currency.clone(),
                                            to_currency: matching_record.currency.clone(),
                                            date_added: Utc::now(),
                                            fees: record
                                                .fee
                                                .parse::<Decimal>()
                                                .unwrap_or(dec!(0.0)),
                                        };
                                        add_fx_conversion_to_db(fx_conversion).await?;
                                    }
                                }
                            };
                        }
                    }
                    AccountRecordType::Unmatched => continue,
                }
            }
        }
    }

    Ok(())
}
