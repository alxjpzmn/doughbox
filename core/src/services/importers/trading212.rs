use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{io::Cursor, str::FromStr};

use chrono::{NaiveDate, Utc};
use csv::{ReaderBuilder, StringRecord};
use serde::Deserialize;

use crate::util::{
    db_helpers::{
        add_dividend_to_db, add_fx_conversion_to_db, add_interest_to_db, add_trade_to_db, Dividend,
        FxConversion, InterestPayment, Trade,
    },
    general_helpers::parse_timestamp,
    market_data_helpers::convert_amount,
};

#[derive(Debug, Deserialize)]
pub struct Trading212RecordV1 {
    pub action: String,
    pub time: String,
    pub isin: String,
    pub _ticker: String,
    pub _name: String,
    pub share_count: String,
    pub price_per_share: String,
    pub currency: String,
    pub fx_rate: String,
    pub _result: String,
    pub _currency_result: String,
    pub total: String,
    pub currency_total: String,
    pub withholding_tax: String,
    pub currency_withholding_tax: String,
    pub _notes: String,
    pub id: String,
    pub currency_conversion_fee: String,
    pub _currency_currency_conversion_fee: String,
}

#[derive(Debug, Deserialize)]
pub struct Trading212RecordV2 {
    action: String,
    time: String,
    isin: String,
    _ticker: String,
    _name: String,
    share_count: String,
    price_per_share: String,
    currency: String,
    fx_rate: String,
    _result: String,
    _currency_result: String,
    total: String,
    currency_total: String,
    withholding_tax: String,
    currency_withholding_tax: String,
    _charge_amount: String,
    _currency_charge_amount: String,
    _deposit_fee: String,
    _currency_deposit_fee: String,
    _notes: String,
    id: String,
    currency_conversion_from_amount: String,
    currency_currency_conversion_from_amount: String,
    currency_conversion_to_amount: String,
    currency_currency_conversion_to_amount: String,
    currency_conversion_fee: String,
    _currency_currency_conversion_fee: String,
}

#[derive(Debug, Deserialize)]
pub struct Trading212RecordV3 {
    action: String,
    time: String,
    isin: String,
    _ticker: String,
    _name: String,
    share_count: String,
    price_per_share: String,
    currency_price_per_share: String,
    exchange_rate: String,
    _result: String,
    _currency_result: String,
    total: String,
    currency_total: String,
    withholding_tax: String,
    currency_withholding_tax: String,
    _stamp_duty: String,
    _currency_stamp_duty: String,
    _stamp_duty_reserve_tax: String,
    _currency_stamp_duty_reserve_tax: String,
    _notes: String,
    id: String,
    currency_conversion_fee: String,
    _currency_currency_conversion_fee: String,
    _merchant_name: String,
    _merchant_category: String,
}

#[derive(Debug, Deserialize)]
pub struct Trading212RecordV4 {
    action: String,
    time: String,
    isin: String,
    _ticker: String,
    _name: String,
    share_count: String,
    price_per_share: String,
    currency_price_per_share: String,
    exchange_rate: String,
    _result: String,
    _currency_result: String,
    total: String,
    currency_total: String,
    withholding_tax: String,
    currency_withholding_tax: String,
    _charge_amount: String,
    _currency_charge_amount: String,
    _deposit_fee: String,
    _currency_deposit_fee: String,
    _stamp_duty: String,
    _currency_stamp_duty: String,
    _stamp_duty_reserve_tax: String,
    _currency_stamp_duty_reserve_tax: String,
    _notes: String,
    id: String,
    currency_conversion_from_amount: String,
    currency_currency_conversion_from_amount: String,
    currency_conversion_to_amount: String,
    currency_currency_conversion_to_amount: String,
    currency_conversion_fee: String,
    _currency_currency_conversion_fee: String,
    _merchant_name: String,
    _merchant_category: String,
}

enum RecordType {
    Dividend,
    EquityTrade,
    CashInterest,
    ShareInterest,
    CashTransfer,
    FxConversion,
    Unmatched,
}

fn detect_legacy_record_type(record: &Trading212RecordV1) -> RecordType {
    if record.action.contains("Dividend") {
        return RecordType::Dividend;
    }
    if record.action == "Withdrawal" || record.action == "Deposit" {
        return RecordType::CashTransfer;
    }
    if record.action == "Interest on cash" {
        return RecordType::CashInterest;
    }
    if record.action == "Interest on cash" {
        return RecordType::CashInterest;
    }
    if record.action == "Lending interest" {
        return RecordType::ShareInterest;
    }
    if record.action.contains("buy") || record.action.contains("sell") {
        return RecordType::EquityTrade;
    }
    if record.action == "Currency conversion" {
        return RecordType::FxConversion;
    }
    RecordType::Unmatched
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

#[derive(Debug)]
enum RecordVersion {
    V1,
    V2,
    V3,
    V4,
}

fn detect_csv_version(headers: &StringRecord) -> RecordVersion {
    match headers.len() {
        33 => RecordVersion::V4,
        27 => RecordVersion::V2,
        25 => RecordVersion::V3,
        19 => RecordVersion::V1,
        _ => panic!(
            "Can't identify Trading 212 record version, found {:?} columns:{:?}",
            headers.len(),
            headers
        ),
    }
}

pub async fn extract_trading212_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Trading212".to_string();

    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(cursor);

    let version = detect_csv_version(rdr.headers().unwrap());

    match version {
        RecordVersion::V1 => {
            for result in rdr.deserialize() {
                let record: Trading212RecordV1 = result?;

                let record_type = detect_legacy_record_type(&record);

                match record_type {
                    RecordType::FxConversion => continue,
                    RecordType::Dividend => {
                        let dividend = Dividend {
                            isin: record.isin,
                            date: parse_timestamp(&record.time)?,
                            amount: record.share_count.parse::<Decimal>()?
                                * record.price_per_share.parse::<Decimal>()?,
                            broker: broker.clone(),
                            currency: record.currency,
                            // Trading 212 always lists the amount in EUR for dividends in the Total column
                            amount_eur: record.total.parse::<Decimal>()?,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_dividend_to_db(dividend).await?;
                    }
                    RecordType::EquityTrade => {
                        let trade = Trade {
                            broker: broker.clone(),
                            date: parse_timestamp(&record.time)?,
                            isin: record.isin,
                            avg_price_per_unit: record.price_per_share.parse::<Decimal>()?,
                            eur_avg_price_per_unit: record.price_per_share.parse::<Decimal>()?
                                / record.fx_rate.parse::<Decimal>()?,
                            no_units: if record.price_per_share.is_empty() {
                                dec!(0.0)
                            } else {
                                record.share_count.parse::<Decimal>()?
                            },
                            direction: if record.action.contains("buy") {
                                "Buy".to_string()
                            } else {
                                "Sell".to_string()
                            },
                            security_type: "Equity".to_string(),
                            currency_denomination: record.currency.to_string(),
                            date_added: Utc::now(),
                            // Trading 212 only charges for fx conversions
                            fees: record
                                .currency_conversion_fee
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_trade_to_db(trade, Some(record.id)).await?;
                    }
                    RecordType::CashInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &NaiveDate::from_str(
                                    parse_timestamp(&record.time)?
                                        .date_naive()
                                        .to_string()
                                        .as_str(),
                                )?,
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Cash".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::ShareInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &NaiveDate::from_str(
                                    parse_timestamp(&record.time)?
                                        .date_naive()
                                        .to_string()
                                        .as_str(),
                                )?,
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Shares".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::CashTransfer => continue,
                    RecordType::Unmatched => continue,
                }
            }
        }

        RecordVersion::V2 => {
            for result in rdr.deserialize() {
                let record: Trading212RecordV2 = result?;
                let record_type = detect_record_type(&record.action);

                match record_type {
                    RecordType::FxConversion => {
                        let fx_conversion = FxConversion {
                            date: parse_timestamp(&record.time)?,
                            broker: broker.clone(),
                            from_amount: record
                                .currency_conversion_from_amount
                                .parse::<Decimal>()?,
                            to_amount: record.currency_conversion_to_amount.parse::<Decimal>()?,
                            from_currency: record.currency_currency_conversion_from_amount,
                            to_currency: record.currency_currency_conversion_to_amount,
                            date_added: Utc::now(),
                            // fee is always in EUR
                            fees: record
                                .currency_conversion_fee
                                .parse::<Decimal>()
                                .unwrap_or(dec!(-0.0))
                                * -dec!(1.0),
                        };
                        add_fx_conversion_to_db(fx_conversion).await?;
                    }
                    RecordType::Dividend => {
                        let dividend = Dividend {
                            isin: record.isin,
                            date: parse_timestamp(&record.time)?,
                            amount: record.share_count.parse::<Decimal>()?
                                * record.price_per_share.parse::<Decimal>()?,
                            broker: broker.clone(),
                            currency: record.currency,
                            // Trading 212 always lists the amount in EUR for dividends in the Total column
                            amount_eur: record.total.parse::<Decimal>()?,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_dividend_to_db(dividend).await?;
                    }
                    RecordType::EquityTrade => {
                        let trade = Trade {
                            broker: broker.clone(),
                            date: parse_timestamp(&record.time)?,
                            isin: record.isin,
                            avg_price_per_unit: record.price_per_share.parse::<Decimal>()?,
                            eur_avg_price_per_unit: record.price_per_share.parse::<Decimal>()?
                                / record.fx_rate.parse::<Decimal>()?,
                            no_units: if record.price_per_share.is_empty() {
                                dec!(0.0)
                            } else {
                                record.share_count.parse::<Decimal>()?
                            },
                            direction: if record.action.contains("buy") {
                                "Buy".to_string()
                            } else {
                                "Sell".to_string()
                            },
                            security_type: "Equity".to_string(),
                            currency_denomination: record.currency.to_string(),
                            date_added: Utc::now(),
                            // Trading 212 only charges for fx conversions
                            fees: record
                                .currency_conversion_fee
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_trade_to_db(trade, Some(record.id)).await?;
                    }
                    RecordType::CashInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &parse_timestamp(&record.time)?.date_naive(),
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Cash".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::ShareInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &NaiveDate::from_str(
                                    parse_timestamp(&record.time)?
                                        .date_naive()
                                        .to_string()
                                        .as_str(),
                                )?,
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Shares".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::CashTransfer => continue,
                    RecordType::Unmatched => continue,
                }
            }
        }
        RecordVersion::V3 => {
            for result in rdr.deserialize() {
                let record: Trading212RecordV3 = result?;
                let record_type = detect_record_type(&record.action);

                match record_type {
                    RecordType::FxConversion => {}
                    RecordType::Dividend => {
                        let dividend = Dividend {
                            isin: record.isin,
                            date: parse_timestamp(&record.time)?,
                            amount: record.share_count.parse::<Decimal>()?
                                * record.price_per_share.parse::<Decimal>()?,
                            broker: broker.clone(),
                            currency: record.currency_price_per_share,
                            // Trading 212 always lists the amount in EUR for dividends in the Total column
                            amount_eur: record.total.parse::<Decimal>()?,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_dividend_to_db(dividend).await?;
                    }
                    RecordType::EquityTrade => {
                        let trade = Trade {
                            broker: broker.clone(),
                            date: parse_timestamp(&record.time)?,
                            isin: record.isin,
                            avg_price_per_unit: record.price_per_share.parse::<Decimal>()?,
                            eur_avg_price_per_unit: record.price_per_share.parse::<Decimal>()?
                                / record.exchange_rate.parse::<Decimal>()?,
                            no_units: if record.price_per_share.is_empty() {
                                dec!(0.0)
                            } else {
                                record.share_count.parse::<Decimal>()?
                            },
                            direction: if record.action.contains("buy") {
                                "Buy".to_string()
                            } else {
                                "Sell".to_string()
                            },
                            security_type: "Equity".to_string(),
                            currency_denomination: record.currency_price_per_share.to_string(),
                            date_added: Utc::now(),
                            // Trading 212 only charges for fx conversions
                            fees: record
                                .currency_conversion_fee
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_trade_to_db(trade, Some(record.id)).await?;
                    }
                    RecordType::CashInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &parse_timestamp(&record.time)?.date_naive(),
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Cash".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::ShareInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &NaiveDate::from_str(
                                    parse_timestamp(&record.time)?
                                        .date_naive()
                                        .to_string()
                                        .as_str(),
                                )?,
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Shares".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::CashTransfer => continue,
                    RecordType::Unmatched => continue,
                }
            }
        }
        RecordVersion::V4 => {
            for result in rdr.deserialize() {
                let record: Trading212RecordV4 = result?;
                let record_type = detect_record_type(&record.action);

                match record_type {
                    RecordType::FxConversion => {
                        let fx_conversion = FxConversion {
                            date: parse_timestamp(&record.time)?,
                            broker: broker.clone(),
                            from_amount: record
                                .currency_conversion_from_amount
                                .parse::<Decimal>()?,
                            to_amount: record.currency_conversion_to_amount.parse::<Decimal>()?,
                            from_currency: record.currency_currency_conversion_from_amount,
                            to_currency: record.currency_currency_conversion_to_amount,
                            date_added: Utc::now(),
                            // fee is always in EUR
                            fees: record
                                .currency_conversion_fee
                                .parse::<Decimal>()
                                .unwrap_or(dec!(-0.0))
                                * -dec!(1.0),
                        };
                        add_fx_conversion_to_db(fx_conversion).await?;
                    }
                    RecordType::Dividend => {
                        let dividend = Dividend {
                            isin: record.isin,
                            date: parse_timestamp(&record.time)?,
                            amount: record.share_count.parse::<Decimal>()?
                                * record.price_per_share.parse::<Decimal>()?,
                            broker: broker.clone(),
                            currency: record.currency_price_per_share,
                            // Trading 212 always lists the amount in EUR for dividends in the Total column
                            amount_eur: record.total.parse::<Decimal>()?,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_dividend_to_db(dividend).await?;
                    }
                    RecordType::EquityTrade => {
                        let trade = Trade {
                            broker: broker.clone(),
                            date: parse_timestamp(&record.time)?,
                            isin: record.isin,
                            avg_price_per_unit: record.price_per_share.parse::<Decimal>()?,
                            eur_avg_price_per_unit: record.price_per_share.parse::<Decimal>()?
                                / record.exchange_rate.parse::<Decimal>()?,
                            no_units: if record.price_per_share.is_empty() {
                                dec!(0.0)
                            } else {
                                record.share_count.parse::<Decimal>()?
                            },
                            direction: if record.action.contains("buy") {
                                "Buy".to_string()
                            } else {
                                "Sell".to_string()
                            },
                            security_type: "Equity".to_string(),
                            currency_denomination: record.currency_price_per_share.to_string(),
                            date_added: Utc::now(),
                            // Trading 212 only charges for fx conversions
                            fees: record
                                .currency_conversion_fee
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_trade_to_db(trade, Some(record.id)).await?;
                    }
                    RecordType::CashInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &parse_timestamp(&record.time)?.date_naive(),
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Cash".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::ShareInterest => {
                        let amount = if record.currency_total == "EUR" {
                            record.total.parse::<Decimal>()?
                        } else {
                            convert_amount(
                                record.total.parse::<Decimal>()?,
                                &NaiveDate::from_str(
                                    parse_timestamp(&record.time)?
                                        .date_naive()
                                        .to_string()
                                        .as_str(),
                                )?,
                                &record.currency_total,
                                "EUR",
                            )
                            .await?
                        };

                        let interest_payment = InterestPayment {
                            date: parse_timestamp(&record.time)?,
                            amount: record.total.parse::<Decimal>()?,
                            broker: broker.clone(),
                            principal: "Shares".to_string(),
                            currency: record.currency_total,
                            amount_eur: amount,
                            withholding_tax: record
                                .withholding_tax
                                .parse::<Decimal>()
                                .unwrap_or(dec!(0.0)),
                            witholding_tax_currency: record.currency_withholding_tax,
                        };
                        add_interest_to_db(interest_payment).await?;
                    }
                    RecordType::CashTransfer => continue,
                    RecordType::Unmatched => continue,
                }
            }
        }
    }

    Ok(())
}
