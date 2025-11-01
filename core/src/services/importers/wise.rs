use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::io::Cursor;

use chrono::prelude::*;
use csv::{ReaderBuilder, StringRecord};
use serde::Deserialize;

use crate::{
    database::{
        models::{fx_conversion::FxConversion, interest::InterestPayment, trade::Trade},
        queries::{
            composite::add_trade_to_db, fx_conversion::add_fx_conversion_to_db,
            interest::add_interest_to_db,
        },
    },
    services::{market_data::fx_rates::convert_amount, parsers::parse_timestamp},
};

#[derive(Debug, Deserialize)]
pub struct WiseCashRecord {
    _transferwise_id: String,
    date: String,
    amount: String,
    currency: String,
    description: String,
    _payment_reference: String,
    _running_balance: String,
    exchange_from: String,
    exchange_to: String,
    _exchange_rate: String,
    _payer_name: String,
    _payee_name: String,
    _payee_account_number: String,
    _merchant: String,
    _card_last_four_digits: String,
    _card_holder_full_name: String,
    _attachment: String,
    _note: String,
    total_fees: String,
    exchange_to_amount: String,
}

#[derive(Debug, Deserialize)]
pub struct WiseAssetRecord {
    traded_asset_id_type: String,
    traded_asset_id_value: String,
    execution_date: String,
    transaction_type: String,
    traded_units: String,
    asset_base_currency: String,
    asset_base_currency_unit_price_amount: String,
    _asset_base_currency_value_traded: String,
    _settlement_date: String,
    _settlement_currency: String,
    _settlement_amount: String,
    _settlement_conversion_rate: String,
    _settlement_conversion_rate_timestamp: String,
    _legal_entity: String,
    wise_id: String,
}

#[derive(Debug, Deserialize)]
pub struct WiseCashLegacyRecord {
    _transferwise_id: String,
    date: String,
    amount: String,
    currency: String,
    description: String,
    _payment_reference: String,
    _running_balance: String,
    exchange_from: String,
    exchange_to: String,
    exchange_rate: String,
    _payer_name: String,
    _payee_name: String,
    _payee_account_number: String,
    _merchant: String,
    _card_last_four_digits: String,
    _card_holder_full_name: String,
    _attachment: String,
    _note: String,
    total_fees: String,
    exchange_to_amount: String,
    _asset_base_currency_unit_price_amount_0: String,
    _asset_base_currency_unit_price_currency_0: String,
    _effective_trade_rate_0: String,
    _settlement_currency_unit_price_amount_0: String,
    _settlement_currency_unit_price_currency_0: String,
    _trade_side_0: String,
    _traded_asset_id_type_0: String,
    _traded_asset_id_value_0: String,
    _traded_units_0: String,
}

#[derive(Debug)]
enum StatementType {
    Asset,
    Cash,
    CashLegacy,
}

fn detect_csv_version(headers: &StringRecord) -> StatementType {
    if headers.len() == 20 {
        return StatementType::Cash;
    }
    if headers.len() == 29 {
        StatementType::CashLegacy
    } else {
        StatementType::Asset
    }
}

enum CashRecordType {
    FxConversion,
    InterestPayment,
    Unmatched,
}

enum AssetRecordTye {
    EquityTrade,
    Unmatched,
}

fn detect_cash_record_type(record: &WiseCashRecord) -> CashRecordType {
    if !record.exchange_to_amount.is_empty() && record.exchange_to_amount != "Exchange To Amount" {
        return CashRecordType::FxConversion;
    }
    if record.description == "Balance cashback" && record.exchange_to_amount != "Exchange To Amount"
    {
        return CashRecordType::InterestPayment;
    }
    CashRecordType::Unmatched
}

fn detect_legacy_cash_record_type(record: &WiseCashLegacyRecord) -> CashRecordType {
    if !record.exchange_to_amount.is_empty() && record.exchange_to_amount != "Exchange To Amount" {
        return CashRecordType::FxConversion;
    }
    if record.description == "Balance cashback" && record.exchange_to_amount != "Exchange To Amount"
    {
        return CashRecordType::InterestPayment;
    }
    CashRecordType::Unmatched
}

fn detect_asset_record_type(record: &WiseAssetRecord) -> AssetRecordTye {
    if record.traded_asset_id_type == "ISIN" {
        return AssetRecordTye::EquityTrade;
    }
    AssetRecordTye::Unmatched
}

pub async fn extract_wise_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Wise".to_string();

    let cursor = Cursor::new(file_content);

    let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(cursor);

    let statement_type = detect_csv_version(rdr.headers().unwrap());

    match statement_type {
        StatementType::Asset => {
            for result in rdr.deserialize() {
                let record: WiseAssetRecord = result?;
                let record_type = detect_asset_record_type(&record);
                match record_type {
                    AssetRecordTye::EquityTrade => {
                        let date = parse_timestamp(&record.execution_date)?;
                        let trade = Trade {
                            broker: broker.clone(),
                            date,
                            units: record.traded_units.parse::<Decimal>()?,
                            avg_price_per_unit: record
                                .asset_base_currency_unit_price_amount
                                .parse::<Decimal>()?,
                            eur_avg_price_per_unit: if record.asset_base_currency == "EUR" {
                                record
                                    .asset_base_currency_unit_price_amount
                                    .parse::<Decimal>()?
                            } else {
                                convert_amount(
                                    record
                                        .asset_base_currency_unit_price_amount
                                        .parse::<Decimal>()?,
                                    &date.date_naive(),
                                    &record.asset_base_currency,
                                    "EUR",
                                )
                                .await?
                            },
                            security_type: "Equity".to_string(),
                            direction: if record.transaction_type == "BUY" {
                                "Buy".to_string()
                            } else {
                                "Sell".to_string()
                            },
                            currency: record.asset_base_currency.clone(),
                            isin: record.traded_asset_id_value,
                            date_added: Utc::now(),
                            // Wise charges a custody fee instead
                            fees: dec!(0.0),
                            withholding_tax: dec!(0.0),
                            withholding_tax_currency: record.asset_base_currency.clone(),
                        };
                        add_trade_to_db(trade, Some(record.wise_id)).await?;
                    }
                    AssetRecordTye::Unmatched => continue,
                }
            }
        }
        StatementType::CashLegacy => {
            for result in rdr.deserialize() {
                let record: WiseCashLegacyRecord = result?;
                let record_type = detect_legacy_cash_record_type(&record);
                if record.date == *"Date" {
                    continue;
                }
                let date = parse_timestamp(format!("{} 16:00:00", record.date).as_str())?;
                match record_type {
                    CashRecordType::FxConversion => {
                        let fx_conversion = FxConversion {
                            date,
                            broker: broker.clone(),
                            from_amount: record.amount.parse::<Decimal>()?
                                / record.exchange_rate.parse::<Decimal>()?,
                            to_amount: record.exchange_to_amount.parse::<Decimal>()?,
                            from_currency: record.exchange_from,
                            to_currency: record.exchange_to,
                            date_added: Utc::now(),
                            fees: record.total_fees.parse::<Decimal>()?,
                        };
                        add_fx_conversion_to_db(fx_conversion).await?
                    }
                    CashRecordType::InterestPayment => {
                        let date = parse_timestamp(format!("{} 16:00:00", record.date).as_str())?;
                        let interest_payment = InterestPayment {
                            date,
                            broker: broker.clone(),
                            amount: record.amount.parse::<Decimal>()?,
                            principal: "Cash".to_string(),
                            currency: record.currency.clone(),
                            amount_eur: if record.currency == "EUR" {
                                record.amount.parse::<Decimal>()?
                            } else {
                                convert_amount(
                                    record.amount.parse::<Decimal>()?,
                                    &date.date_naive(),
                                    &record.currency,
                                    "EUR",
                                )
                                .await?
                            },
                            // Wise retains a withholding tax of 30% (Belgium),
                            // but doesn't add it to the statement
                            // the amount in the statement is actually already the net amount
                            withholding_tax: dec!(0.0),
                            withholding_tax_currency: record.currency,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    CashRecordType::Unmatched => continue,
                }
            }
        }
        StatementType::Cash => {
            for result in rdr.deserialize() {
                let record: WiseCashRecord = result?;
                let record_type = detect_cash_record_type(&record);
                if record.date == *"Date" {
                    continue;
                }
                let date = parse_timestamp(format!("{} 16:00:00", record.date).as_str())?;
                match record_type {
                    CashRecordType::FxConversion => {
                        let fx_conversion = FxConversion {
                            date,
                            broker: broker.clone(),
                            from_amount: record.amount.parse::<Decimal>()? * dec!(-1.0),
                            to_amount: record.exchange_to_amount.parse::<Decimal>()?,
                            from_currency: record.exchange_from,
                            to_currency: record.exchange_to,
                            date_added: Utc::now(),
                            fees: record.total_fees.parse::<Decimal>()?,
                        };
                        add_fx_conversion_to_db(fx_conversion).await?
                    }
                    CashRecordType::InterestPayment => {
                        let interest_payment = InterestPayment {
                            date,
                            broker: broker.clone(),
                            amount: record.amount.parse::<Decimal>()?,
                            principal: "Cash".to_string(),
                            currency: record.currency.clone(),
                            amount_eur: if record.currency == "EUR" {
                                record.amount.parse::<Decimal>()?
                            } else {
                                convert_amount(
                                    record.amount.parse::<Decimal>()?,
                                    &date.date_naive(),
                                    &record.currency,
                                    "EUR",
                                )
                                .await?
                            },
                            // Wise retains a withholding tax of 30% (Belgium),
                            // but doesn't add it to the statement
                            // the amount in the statement is actually already the net amount
                            withholding_tax: dec!(0.0),
                            withholding_tax_currency: record.currency,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    CashRecordType::Unmatched => continue,
                }
            }
        }
    }
    Ok(())
}
