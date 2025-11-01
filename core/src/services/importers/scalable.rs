use chrono::prelude::*;
use logos::Logos;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::{
    database::{
        models::{dividend::Dividend, trade::Trade},
        queries::{composite::add_trade_to_db, dividend::add_dividend_to_db},
    },
    services::parsers::{does_match_exist, parse_timestamp},
};

#[derive(Debug)]
enum RecordType {
    EquityTrade,
    Dividend,
    Unmatched,
}

fn detect_record_type(text: &str) -> RecordType {
    if does_match_exist(r"Dividendenabrechnung", text) {
        RecordType::Dividend
    } else if does_match_exist(r"Wertpapierabrechnung", text) {
        RecordType::EquityTrade
    } else {
        RecordType::Unmatched
    }
}

#[derive(Logos, Debug, PartialEq)]
enum TradeToken {
    #[regex(r"Auftragszeit", priority = 2)]
    DateKeyword,
    #[regex(r"ISIN", priority = 2)]
    IsinKeyword,
    #[regex(r"NominaleSTK", priority = 2)]
    SharesKeyword,
    #[regex(r"EURKurswert", priority = 2)]
    PriceKeyword,
    #[regex(r"\d{2}\.\d{2}\.\d{4}\d{2}:\d{2}:\d{2}", priority = 1)]
    Date,
    #[regex(r"[A-Z]{2}[A-Z0-9]{9}\d")]
    Isin,
    #[regex(r"\d+(\.\d{1,3})?", priority = 2)]
    Number,
    #[regex(r"\d{1,3}(?:[\.,]\d{3})*(?:[\.,]\d{2,3})?", priority = 1)]
    Price,
    #[regex(r"Vorgangs-Nr.", priority = 2)]
    IdKeyword,
    #[regex(r"[:\s]+", logos::skip)]
    Separator,
}

#[derive(Logos, Debug, PartialEq)]
enum DividendToken {
    #[regex(r"Valuta", priority = 2)]
    DateKeyword,
    #[regex(r"ISIN", priority = 2)]
    IsinKeyword,
    #[regex(r"Bruttobetrag EUR", priority = 2)]
    AmountKeyword,
    #[regex(r"\d{2}\.\d{2}\.\d{4}", priority = 1)]
    Date,
    #[regex(r"[A-Z]{2}[A-Z0-9]{9}\d")]
    Isin,
    #[regex(r"\d{1,3}(?:[\.,]\d{2})", priority = 1)]
    Amount,
    #[regex(r"[:\s]+", logos::skip)]
    Separator,
}

fn extract_trade_details(text: &str) -> Option<(String, String, String, String, String)> {
    let mut lexer = TradeToken::lexer(text);
    let mut order_date = None;
    let mut isin = None;
    let mut shares = None;
    let mut price = None;
    let mut id = None;
    let mut last_number = "";

    while let Some(token) = lexer.next() {
        match token {
            Ok(TradeToken::DateKeyword) => {
                while let Some(token) = lexer.next() {
                    if let Ok(TradeToken::Date) = token {
                        order_date = Some(lexer.slice().to_string());
                        break;
                    }
                }
            }
            Ok(TradeToken::IsinKeyword) => {
                if let Some(Ok(TradeToken::Isin)) = lexer.next() {
                    isin = Some(lexer.slice().to_string());
                }
            }
            Ok(TradeToken::SharesKeyword) => {
                if let Some(Ok(TradeToken::Number)) = lexer.next() {
                    shares = Some(lexer.slice().to_string().replace(",", "."));
                }
            }
            Ok(TradeToken::PriceKeyword) => {
                price = Some(last_number.replace(",", "."));
            }
            Ok(TradeToken::Price) => {
                last_number = lexer.slice();
            }
            Ok(TradeToken::IdKeyword) => {
                if let Some(Ok(TradeToken::Number)) = lexer.next() {
                    id = Some(lexer.slice().to_string());
                }
            }
            _ => {}
        }

        if order_date.is_some()
            && isin.is_some()
            && shares.is_some()
            && price.is_some()
            && id.is_some()
        {
            break;
        }
    }

    if let (Some(order_date), Some(isin), Some(shares), Some(price), Some(id)) =
        (order_date, isin, shares, price, id)
    {
        Some((order_date, isin, shares, price, id))
    } else {
        None
    }
}

fn extract_dividend_details(text: &str) -> Option<(String, String, String)> {
    let mut lexer = DividendToken::lexer(text);
    let mut details = (None, None, None);

    while let Some(token) = lexer.next() {
        match token {
            Ok(DividendToken::DateKeyword) => {
                if let Some(Ok(DividendToken::Date)) = lexer.next() {
                    details.0 = Some(lexer.slice().to_string());
                }
            }
            Ok(DividendToken::IsinKeyword) => {
                if let Some(Ok(DividendToken::Isin)) = lexer.next() {
                    details.1 = Some(lexer.slice().to_string());
                }
            }
            Ok(DividendToken::AmountKeyword) => {
                if let Some(Ok(DividendToken::Amount)) = lexer.next() {
                    details.2 = Some(lexer.slice().to_string().replace(",", "."));
                }
            }
            _ => {}
        }
        if details.0.is_some() && details.1.is_some() && details.2.is_some() {
            break;
        }
    }
    match details {
        (Some(date), Some(isin), Some(amount)) => Some((date, isin, amount)),
        _ => None,
    }
}

pub async fn extract_scalable_record(text: &str) -> anyhow::Result<()> {
    let broker = "Scalable".to_string();

    match detect_record_type(text) {
        RecordType::EquityTrade => {
            if let Some((order_date, isin, shares, price, id)) = extract_trade_details(text) {
                let trade = Trade {
                    broker,
                    date: parse_timestamp(&order_date)?,
                    isin,
                    avg_price_per_unit: price.parse::<Decimal>()?,
                    eur_avg_price_per_unit: price.parse::<Decimal>()?,
                    units: shares.parse::<Decimal>()?,
                    direction: if does_match_exist(r"Verkauf", text) {
                        "Sell"
                    } else {
                        "Buy"
                    }
                    .to_string(),
                    security_type: "Equity".to_string(),
                    currency: "EUR".to_string(),
                    date_added: Utc::now(),
                    fees: dec!(0.0),
                    withholding_tax: dec!(0.0),
                    withholding_tax_currency: "EUR".to_string(),
                };
                add_trade_to_db(trade, Some(id)).await?;
            }
        }
        RecordType::Dividend => {
            if let Some((date, isin, amount)) = extract_dividend_details(text) {
                let dividend = Dividend {
                    isin,
                    date: parse_timestamp(&format!("{} 16:00:00", date))?,
                    amount: amount.parse::<Decimal>()?,
                    broker,
                    currency: "EUR".to_string(),
                    amount_eur: amount.parse::<Decimal>()?,
                    withholding_tax: dec!(0.0),
                    withholding_tax_currency: "EUR".to_string(),
                };
                add_dividend_to_db(dividend).await?;
            }
        }
        RecordType::Unmatched => (),
    }
    Ok(())
}
