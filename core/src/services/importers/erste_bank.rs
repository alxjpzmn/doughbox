use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::{
    database::{
        models::{dividend::Dividend, trade::Trade},
        queries::{composite::add_trade_to_db, dividend::add_dividend_to_db},
    },
    services::parsers::{does_match_exist, parse_timestamp, return_first_match},
};
use chrono::prelude::*;

#[derive(Debug)]
enum RecordType {
    EquityTrade,
    Dividend,
    Unmatched,
}

fn detect_record_type(text: &str) -> anyhow::Result<RecordType> {
    if does_match_exist(r"Dividende", text) && does_match_exist(r"ERTRAGS- UND TILGUNGSBELEG", text)
    {
        return Ok(RecordType::Dividend);
    }
    if does_match_exist(r"Kauf Marktplatz", text) || does_match_exist(r"Verkauf Marktplatz", text) {
        return Ok(RecordType::EquityTrade);
    }
    Ok(RecordType::Unmatched)
}

pub async fn extract_erste_bank_record(text: &str) -> anyhow::Result<()> {
    let broker = "Erste Bank".to_string();
    let record_type = detect_record_type(text)?;

    match record_type {
        RecordType::EquityTrade => {
            let date_match =
                return_first_match(r"(..\...\...... ..:..)", text)?.replace(['\n', ','], "");
            let date_string_to_parse = format!("{}:00", date_match);
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin = return_first_match(
                r"\b(?i)([a-z]{2}\d{10})|([a-z]{3}[0-9]{1}[0-9a-z]{9})|([a-z]{2}[0-9]{1}[0-9a-z]{9})|([a-z]{3}[0-9]{1}[0-9a-z]{8})|([a-z]{2}\-\d{9}\-\d)|([a-z]{2}\-[0-9]{1}[0-9a-z]{8}\-\d)|([a-z]{3}\-[0-9]{1}[0-9a-z]{7}\-\d)",
                &text.replace('\n', ""),
            )?;

            let is_non_eur_denominated = does_match_exist(r"Umgerechneter Kurswert", text);

            let avg_price_per_unit = return_first_match(r"STK \d{1,},\d{1,}", text)?
                .replace("STK ", "")
                .replace(" ", "")
                .replace(',', ".")
                .parse::<Decimal>()?;

            let no_units = return_first_match(
                r"(?:\d{2}\.\d{2}\.\d{4})\s*,?\s*(\d{1,3}(?:\.\d{3})*(?:,\d{2})?)\s*STK",
                text,
            )?
            .split_off(10)
            .replace("STK", "")
            .replace(" ", "")
            .replace(".", "")
            .replace(',', ".")
            .parse::<Decimal>()?;

            let order_type = if does_match_exist(r"\sKauf", text) {
                "Buy"
            } else {
                "Sell"
            };

            let mut fees = dec!(0.0);
            if !does_match_exist(
                r"Fur diese Transaktion fielen keine Dienstleistungskosten an.",
                text,
            ) && !does_match_exist("Es sind keine Kosten angefallen.", text)
            {
                fees =
                    return_first_match(r"Summe der Dienstleistungskosten EUR \d{1,},\d{1,}", text)?
                        .replace(',', ".")
                        .replace("Summe der Dienstleistungskosten", "")
                        .replace("EUR", "")
                        .replace(" ", "")
                        .parse::<Decimal>()?;
            };

            let id = return_first_match(r"Auftragsnummer\s*(\S+)", text)?
                .replace("Auftragsnummer", "")
                .replace(" ", "")
                .replace("\n", "");

            let mut currency_denomination = "EUR".to_string();

            if is_non_eur_denominated {
                currency_denomination = return_first_match(r"... Devisenkurs", text)?
                    .replace("Devisenkurs", "")
                    .replace(" ", "");
            }

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
                currency_denomination,
                date_added: Utc::now(),
                fees,
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_trade_to_db(trade, Some(id)).await?;
        }
        RecordType::Dividend => {
            let date_match = return_first_match(r", am \d{2}\.\d{2}\.\d{4}", text)?
                .replace('\n', "")
                .replace(", am", "");
            let date_string_to_parse = format!("{} 16:00:00", date_match);
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin = return_first_match(
                r"\b(?i)([a-z]{2}\d{10})|([a-z]{3}[0-9]{1}[0-9a-z]{9})|([a-z]{2}[0-9]{1}[0-9a-z]{9})|([a-z]{3}[0-9]{1}[0-9a-z]{8})|([a-z]{2}\-\d{9}\-\d)|([a-z]{2}\-[0-9]{1}[0-9a-z]{8}\-\d)|([a-z]{3}\-[0-9]{1}[0-9a-z]{7}\-\d)",
                &text.replace('\n', ""),
            )?;

            let amount = return_first_match(r"(\d{1,3}(?:,\d{3})*)(?:,\d{2})?(?=\s*QESt)", text)?
                .replace("QESt", "")
                .replace(",", ".")
                .replace("\n", "")
                .replace(" ", "")
                .parse::<Decimal>()?;

            let withholding_tax = return_first_match(r"% -\d{1,},\d{1,}", text)?
                .replace("%", "")
                .replace(",", ".")
                .replace(" ", "")
                .replace("-", "")
                .parse::<Decimal>()?;

            let dividend = Dividend {
                isin,
                date,
                broker,
                amount,
                // dividend is immediately converted to EUR
                currency: "EUR".to_string(),
                amount_eur: amount,
                withholding_tax,
                witholding_tax_currency: "EUR".to_string(),
            };
            add_dividend_to_db(dividend).await?;
        }
        RecordType::Unmatched => (),
    }
    Ok(())
}
