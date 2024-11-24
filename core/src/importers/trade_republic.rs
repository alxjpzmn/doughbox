use regex::Regex;
use rust_decimal_macros::dec;
use std::fs;
use std::io;

use crate::util::db_helpers::find_similar_trade;
use crate::util::db_helpers::get_positions_for_isin;
use crate::util::general_helpers::choose_match_from_regex;
use crate::util::general_helpers::hash_string;
use crate::util::{
    db_helpers::{
        add_dividend_to_db, add_interest_to_db, add_trade_to_db, Dividend, InterestPayment, Trade,
    },
    general_helpers::{does_match_exist, parse_timestamp, return_first_match},
};
use chrono::prelude::*;
use rust_decimal::Decimal;

#[derive(Debug)]
enum RecordType {
    EquityTrade,
    Liquidation,
    BondTrade,
    Dividend,
    InvestmentPlanExecution,
    InterestPayment,
    PortfolioTransfer,
}

fn detect_record_type(text: &str) -> anyhow::Result<RecordType> {
    let dividend_patterns = Regex::new(r"(Dividende|COUPON|Aussch.ttung)")?;
    let bond_trade_pattern = Regex::new(r"St.ckzinsen")?;
    let interest_pattern = Regex::new(r"Zinsen")?;
    let liquidation_pattern = Regex::new(r"Tilgung")?;
    let investment_plan_pattern = Regex::new(r"(Sparplanausfuhrung|Saveback)")?;
    let portfolio_transfer_pattern = Regex::new(r"Depot.bertrag")?;

    Ok(match text {
        _ if dividend_patterns.is_match(text) => RecordType::Dividend,
        _ if bond_trade_pattern.is_match(text) => RecordType::BondTrade,
        _ if interest_pattern.is_match(text) => RecordType::InterestPayment,
        _ if liquidation_pattern.is_match(text) => RecordType::Liquidation,
        _ if investment_plan_pattern.is_match(text) => RecordType::InvestmentPlanExecution,
        _ if portfolio_transfer_pattern.is_match(text) => RecordType::PortfolioTransfer,
        _ => RecordType::EquityTrade,
    })
}

// to avoid re-importing on trade id changes
fn require_import_confirmation(trade: &Trade) -> anyhow::Result<bool> {
    let mut input = String::new();
    println!(
        "A similar trade was found with a different ID. Trade details: {:?}. Do you want to import it? (yes/no): ",
        trade
    );
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

async fn add_with_import_confirmation(trade: Trade, id: String) -> anyhow::Result<()> {
    let existing_trade = find_similar_trade(&trade).await?;

    if let Some(existing_trade) = existing_trade {
        if existing_trade.hash != hash_string(&format!("{}{}", &trade.broker, id.as_str()))
            && require_import_confirmation(&trade)?
        {
            add_trade_to_db(trade, Some(id)).await?;
        }
    } else {
        add_trade_to_db(trade, Some(id)).await?;
    }
    Ok(())
}

pub async fn extract_trade_republic_record(text: &str, file_path: &str) -> anyhow::Result<()> {
    // TR supports decimialization of up to 6 decimals
    let no_units_default_regex = r"\d+(,|\.)*\d{0,6}\sStk.";
    let broker = "Trade Republic".to_string();
    match detect_record_type(text)? {
        RecordType::InvestmentPlanExecution => {
            let date_match = return_first_match(r"(..\...\.....)", text)?;
            let date_string_to_parse = format!("{date_match} 16:00:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin =
                return_first_match(r"\b[a-zA-Z]{2}\s*[0-9a-zA-Z]{9}[0-9](?![0-9a-zA-Z-])", text)?;
            let no_units = return_first_match(no_units_default_regex, text)?
                .replace(" Stk.", "")
                .replace(',', ".")
                .parse::<Decimal>()?;

            let mut avg_price_per_unit = return_first_match(r"Stk\.\s\d+,\d{1,}\sEUR", text)?
                .replace(" EUR", "")
                .replace("Stk.", "")
                .replace(',', ".")
                .replace(" ", "")
                .parse::<Decimal>()?;

            let id = return_first_match(r"AUSFUHRUNG\s*(\S+)", text)?
                .replace("AUSFUHRUNG", "")
                .replace(" ", "")
                .replace("\n", "");

            // set price per unit to 0 if saveback
            let is_saveback = does_match_exist("Saveback", text);
            if is_saveback {
                avg_price_per_unit = dec!(0);
            }

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                no_units,
                direction: "Buy".to_string(),
                // Sparplan is only available for Stocks and ETFs on TR
                security_type: "Equity".to_string(),
                // TR only supports EUR
                currency_denomination: "EUR".to_string(),
                date_added: Utc::now(),
                // Sparplan executions are currently always for free
                fees: dec!(0.0),
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_with_import_confirmation(trade, id).await?;
            fs::remove_file(file_path)?;
        }
        RecordType::Liquidation => {
            let date_match = return_first_match(r"(..\...\.....)", text)?;
            // liquidations do not have an hourly time stamp
            let date_string_to_parse = format!("{date_match} 16:00:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin = choose_match_from_regex(r"[A-Z]{2}[A-Z0-9]{9}[0-9]", text)?;

            let is_bond_liquidation = !does_match_exist("Stk.", text);

            let no_units;
            if is_bond_liquidation {
                no_units = get_positions_for_isin(&isin, None).await?;
                if no_units == dec!(0) {
                    return Ok(());
                }
            } else {
                no_units = return_first_match(no_units_default_regex, text)?
                    .replace(" Stk.", "")
                    .replace(',', ".")
                    .parse::<Decimal>()?;
            }

            let avg_price_per_unit =
                return_first_match(r"(\d{1,3}(?:[.,]\d{3})*(?:,\d+)?\s*EUR)", text)?
                    .replace(" EUR", "")
                    .replace(',', ".")
                    .parse::<Decimal>()?
                    / no_units;

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                no_units,
                direction: "Sell".to_string(),
                security_type: if is_bond_liquidation {
                    "Bond".to_string()
                } else {
                    "Derivative".to_string()
                },
                // TR only supports EUR
                currency_denomination: "EUR".to_string(),
                date_added: Utc::now(),
                fees: dec!(0.0),
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_trade_to_db(trade, None).await?;
            fs::remove_file(file_path)?;
        }
        RecordType::BondTrade => {
            let date_match =
                return_first_match(r"(..\...\....., um ..:..)", text)?.replace(", um", "");
            let date_string_to_parse = format!("{date_match}:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin =
                return_first_match(r"\b[a-zA-Z]{2}\s*[0-9a-zA-Z]{9}[0-9](?![0-9a-zA-Z-])", text)?;

            let avg_price_per_unit = return_first_match(r"(\d+,\d+\s*%)", text)?
                .replace(" %", "")
                .replace(',', ".")
                .parse::<Decimal>()?;

            let no_units = return_first_match(
                r"\d+,\d+ EUR",
                &return_first_match(r"(\d+,\d+\s*%)[\s\S]*?(\d+(?:\.\d+)? EUR)", text)?,
            )?
            .replace(" EUR", "")
            .replace(',', ".")
            .parse::<Decimal>()?
                / avg_price_per_unit;

            // get direction
            let direction =
                if does_match_exist(r"\sKauf", text) || does_match_exist(r"\Sparplan", text) {
                    "Buy"
                } else {
                    "Sell"
                };

            let mut fees = dec!(0.0);
            if does_match_exist(r"Fremdkostenzuschlag", text) {
                fees = return_first_match(r"Fremdkostenzuschlag\s(-?\d+,\d{2})\sEUR", text)?
                    .replace("Fremdkostenzuschlag", "")
                    .replace("-", "")
                    .replace(" EUR", "")
                    .replace(" ", "")
                    .replace(',', ".")
                    .parse::<Decimal>()?;
            };

            let id = return_first_match(r"AUSFUHRUNG\s*(\S+)", text)?
                .replace("AUSFUHRUNG", "")
                .replace(" ", "")
                .replace("\n", "");

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                no_units,
                direction: direction.to_string(),
                security_type: "Bond".to_string(),
                // TR only supports EUR
                currency_denomination: "EUR".to_string(),
                date_added: Utc::now(),
                fees,
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_with_import_confirmation(trade, id).await?;
            fs::remove_file(file_path)?;
        }
        RecordType::Dividend => {
            let date_match = return_first_match(r"(..\...\.....)", text)?;
            let date_string_to_parse = format!("{date_match} 16:00:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin =
                return_first_match(r"\b[a-zA-Z]{2}\s*[0-9a-zA-Z]{9}[0-9](?![0-9a-zA-Z-])", text)?;

            let amount = return_first_match(r"EURGESAMT\s+([\d,]+)\s+EUR", text)?
                .replace("EURGESAMT", "")
                .replace(".", "")
                .replace(",", ".")
                .replace("EUR", "")
                .replace("\n", "")
                .replace(" ", "")
                .parse::<Decimal>()?;

            let dividend = Dividend {
                isin,
                date,
                amount,
                broker,
                // Trade Republic only supports EUR
                currency: "EUR".to_string(),
                amount_eur: amount,
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };

            add_dividend_to_db(dividend).await?;
            fs::remove_file(file_path)?;
        }

        RecordType::EquityTrade => {
            let date_match_regex = r"(..\...\....., um ..:..)";
            let does_date_match_exist = does_match_exist(date_match_regex, text);
            //skip file if it's not a valid trade confirmation
            if !does_date_match_exist {
                println!("Skipping {:?}", file_path);
                return Ok(());
            }
            let date_match = return_first_match(date_match_regex, text)?.replace(", um", "");

            let date_string_to_parse = format!("{date_match}:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin_match_regex = r"ISIN:\s([A-Z]{2}[0-9A-Z]{10})";

            let does_isin_match_exist = does_match_exist(isin_match_regex, text);
            if !does_isin_match_exist {
                println!("Skipping {:?}", file_path);
                return Ok(());
            }

            let isin = return_first_match(isin_match_regex, text)?.replace("ISIN: ", "");

            let no_units_match = return_first_match(no_units_default_regex, text)?;
            let no_units = no_units_match
                .replace(" Stk.", "")
                .replace(" EUR", "")
                .replace(" ", "")
                .replace('.', "")
                .replace(',', ".")
                .parse::<Decimal>()?;

            let avg_price_per_unit = return_first_match(r"Stk\.\s\d+,\d{1,}\sEUR", text)?
                .replace(" EUR", "")
                .replace("Stk.", "")
                .replace('.', "")
                .replace(',', ".")
                .replace(" ", "")
                .parse::<Decimal>()?;

            let direction =
                if does_match_exist(r"\sKauf", text) || does_match_exist(r"\Sparplan", text) {
                    "Buy"
                } else {
                    "Sell"
                };

            let mut fees = dec!(0.0);
            if does_match_exist(r"Fremdkostenzuschlag", text) {
                fees = return_first_match(r"Fremdkostenzuschlag\s(-?\d+,\d{2})\sEUR", text)?
                    .replace("Fremdkostenzuschlag", "")
                    .replace("-", "")
                    .replace(" EUR", "")
                    .replace(" ", "")
                    .replace(',', ".")
                    .parse::<Decimal>()?;
            };

            let id = return_first_match(r"AUSFUHRUNG\s*(\S+)", text)?
                .replace("AUSFUHRUNG", "")
                .replace(" ", "")
                .replace("\n", "");

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                no_units,
                direction: direction.to_string(),
                security_type: "Equity".to_string(),
                // TR only supports EUR
                currency_denomination: "EUR".to_string(),
                date_added: Utc::now(),
                fees,
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };
            add_with_import_confirmation(trade, id).await?;
            fs::remove_file(file_path)?;
        }
        RecordType::InterestPayment => {
            let date_match = return_first_match(r"zum (..\...\.....)", text)?.replace("zum ", "");
            let date_string_to_parse = format!("{date_match} 16:00:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let amount = return_first_match(r"\d+,\d{2} EUR", text)?
                .replace("BUCHUNG", "")
                .replace(",", ".")
                .replace("EUR", "")
                .replace("\n", "")
                .replace(" ", "")
                .parse::<Decimal>()?;

            let interest_payment = InterestPayment {
                date,
                amount,
                broker,
                principal: "Cash".to_string(),
                // Trade Republic only supports EUR
                currency: "EUR".to_string(),
                amount_eur: amount,
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                witholding_tax_currency: "EUR".to_string(),
            };

            add_interest_to_db(interest_payment).await?;
            fs::remove_file(file_path)?;
        }
        RecordType::PortfolioTransfer => (),
    }
    Ok(())
}
