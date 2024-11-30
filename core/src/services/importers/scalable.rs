use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::util::{
    db_helpers::{add_dividend_to_db, add_trade_to_db, Dividend, Trade},
    general_helpers::{does_match_exist, parse_timestamp, return_first_match},
};
use chrono::prelude::*;

#[derive(Debug)]
enum RecordType {
    EquityTrade,
    Dividend,
    Unmatched,
}

fn detect_record_type(text: &str) -> RecordType {
    if does_match_exist(r"Dividendenabrechnung", text) {
        return RecordType::Dividend;
    }
    if does_match_exist(r"Wertpapierabrechnung", text) {
        return RecordType::EquityTrade;
    }
    RecordType::Unmatched
}

pub async fn extract_scalable_record(text: &str) -> anyhow::Result<()> {
    let broker = "Scalable".to_string();
    let record_type = detect_record_type(text);

    match record_type {
        RecordType::EquityTrade => {
            let date_match =
                return_first_match(r"(\d{2}\.\d{2}\.\d{4})\s+(\d{2}:\d{2}:\d{2}:\d{2})", text)?;
            let date_string_to_parse = date_match;
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin = return_first_match(
                r"\b(?i)([a-z]{2}\d{10})|([a-z]{3}[0-9]{1}[0-9a-z]{9})|([a-z]{2}[0-9]{1}[0-9a-z]{9})|([a-z]{3}[0-9]{1}[0-9a-z]{8})|([a-z]{2}\-\d{9}\-\d)|([a-z]{2}\-[0-9]{1}[0-9a-z]{8}\-\d)|([a-z]{3}\-[0-9]{1}[0-9a-z]{7}\-\d)",
                &text.replace('\n', ""),
            )?;

            let avg_price_per_unit =
                return_first_match(r"EUR\s+(\d{1,3}(?:\.\d{3})*,\d{0,})Verwahrart", text)?
                    .replace("EUR", "")
                    .replace("Verwahrart", "")
                    .replace(",", ".")
                    .replace("\n", "")
                    .replace(" ", "")
                    .parse::<Decimal>()?;

            let no_units = return_first_match(r"STK \d+,*\d{0,}", text)?
                .replace("STK", "")
                .replace(" ", "")
                .replace(",", ".")
                .parse::<Decimal>()?;

            let order_type = if does_match_exist(r"Verkauf", text) {
                "Sell"
            } else {
                "Buy"
            };

            let id = return_first_match(r"Vorgangs-Nr\.\s*:\s*(\d+)", text)?
                .replace("Vorgangs-Nr.: ", "")
                .replace(" ", "")
                .replace("\n", "");

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // EUR price per unit is shown in PDF itself
                eur_avg_price_per_unit: avg_price_per_unit,
                no_units,
                direction: order_type.to_string(),
                security_type: "Equity".to_string(),
                currency_denomination: "EUR".to_string(),
                date_added: Utc::now(),
                // Scalable has fees, but doesn't include them in their trade confirmation
                // PDF.
                fees: dec!(0.0),
                // Scalable doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_trade_to_db(trade, Some(id)).await?;
        }
        RecordType::Dividend => {
            let date_match =
                return_first_match(r"Valuta: (..\...\.....)", text)?.replace("Valuta: ", "");
            let date_string_to_parse = format!("{} 16:00:00", date_match);
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin = return_first_match(
                r"\b(?i)([a-z]{2}\d{10})|([a-z]{3}[0-9]{1}[0-9a-z]{9})|([a-z]{2}[0-9]{1}[0-9a-z]{9})|([a-z]{3}[0-9]{1}[0-9a-z]{8})|([a-z]{2}\-\d{9}\-\d)|([a-z]{2}\-[0-9]{1}[0-9a-z]{8}\-\d)|([a-z]{3}\-[0-9]{1}[0-9a-z]{7}\-\d)",
                &text.replace('\n', ""),
            )?;

            let amount = return_first_match(r"Bruttobetrag EUR \d+,\d{2}", text)?
                .replace("Bruttobetrag", "")
                .replace("EUR", "")
                .replace(",", ".")
                .replace(" ", "")
                .parse::<Decimal>()?;

            let dividend = Dividend {
                isin,
                date,
                amount,
                broker,
                // Scalable only supports EUR
                currency: "EUR".to_string(),
                amount_eur: amount,
                // Scalable doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_dividend_to_db(dividend).await?;
        }
        RecordType::Unmatched => (),
    }
    Ok(())
}
