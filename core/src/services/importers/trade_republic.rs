use log::info;
use regex::Regex;
use rust_decimal_macros::dec;
use std::io;

use crate::cli::import::choose_match_from_regex;
use crate::database::models::dividend::Dividend;
use crate::database::models::interest::InterestPayment;
use crate::database::models::trade::Trade;
use crate::database::queries::composite::add_trade_to_db;
use crate::database::queries::dividend::add_dividend_to_db;
use crate::database::queries::interest::add_interest_to_db;
use crate::database::queries::position::get_positions_for_isin;
use crate::database::queries::trade::find_similar_trade;
use crate::services::parsers::does_match_exist;
use crate::services::parsers::parse_timestamp;
use crate::services::parsers::return_first_match;
use crate::services::shared::util::hash_string;
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
    Unmatched,
}

fn detect_record_type(text: &str) -> anyhow::Result<RecordType> {
    let dividend_patterns = Regex::new(r"(Dividende|COUPON|Ausschuttung)")?;
    let bond_trade_pattern = Regex::new(r"Stuckzinsen")?;
    let interest_pattern = Regex::new(r"Zinsen")?;
    let liquidation_pattern = Regex::new(r"Tilgung")?;
    let investment_plan_pattern = Regex::new(r"(Sparplanausfuhrung|Saveback)")?;
    let portfolio_transfer_pattern = Regex::new(r"Depotubertrag")?;
    let trade_patterns = Regex::new(r"Market-Order|Limit-Order|Stop-Market-Order")?;

    Ok(match text {
        _ if dividend_patterns.is_match(text) => RecordType::Dividend,
        _ if bond_trade_pattern.is_match(text) => RecordType::BondTrade,
        _ if interest_pattern.is_match(text) => RecordType::InterestPayment,
        _ if liquidation_pattern.is_match(text) => RecordType::Liquidation,
        _ if investment_plan_pattern.is_match(text) => RecordType::InvestmentPlanExecution,
        _ if portfolio_transfer_pattern.is_match(text) => RecordType::PortfolioTransfer,
        _ if trade_patterns.is_match(text) => RecordType::EquityTrade,
        _ => RecordType::Unmatched,
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

pub async fn extract_trade_republic_record(text: &str) -> anyhow::Result<()> {
    // TR supports decimialization of up to 6 decimals
    let units_default_regex = r"\d+(,|\.)*\d{0,6}\sStk.";
    let broker = "Trade Republic".to_string();
    match detect_record_type(text)? {
        RecordType::InvestmentPlanExecution => {
            let date_match = return_first_match(r"(..\...\.....)", text)?;
            let date_string_to_parse = format!("{date_match} 16:00:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin =
                return_first_match(r"\b[a-zA-Z]{2}\s*[0-9a-zA-Z]{9}[0-9](?![0-9a-zA-Z-])", text)?;
            let units = return_first_match(units_default_regex, text)?
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
                units,
                direction: "Buy".to_string(),
                // Sparplan is only available for Stocks and ETFs on TR
                security_type: "Equity".to_string(),
                // TR only supports EUR
                currency: "EUR".to_string(),
                date_added: Utc::now(),
                // Sparplan executions are currently always for free
                fees: dec!(0.0),
                // TR doesn't withhold any tax
                withholding_tax: dec!(0.0),
                withholding_tax_currency: "EUR".to_string(),
            };
            add_with_import_confirmation(trade, id).await?;
        }
        RecordType::Liquidation => {
            let date_match = return_first_match(r"(..\...\.....)", text)?;
            // liquidations do not have an hourly time stamp
            let date_string_to_parse = format!("{date_match} 16:00:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin = choose_match_from_regex(r"[A-Z]{2}[A-Z0-9]{9}[0-9]", text)?;

            let is_bond_liquidation = !does_match_exist("Stk.", text);

            let units;
            if is_bond_liquidation {
                units = get_positions_for_isin(&isin, None).await?;
                if units == dec!(0) {
                    return Ok(());
                }
            } else {
                units = return_first_match(units_default_regex, text)?
                    .replace(" Stk.", "")
                    .replace(',', ".")
                    .parse::<Decimal>()?;
            }

            let avg_price_per_unit =
                return_first_match(r"(\d{1,3}(?:[.,]\d{3})*(?:,\d+)?\s*EUR)", text)?
                    .replace(" EUR", "")
                    .replace(',', ".")
                    .parse::<Decimal>()?
                    / units;

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                units,
                direction: "Sell".to_string(),
                security_type: if is_bond_liquidation {
                    "Bond".to_string()
                } else {
                    "Derivative".to_string()
                },
                // TR only supports EUR
                currency: "EUR".to_string(),
                date_added: Utc::now(),
                fees: dec!(0.0),
                // TR doesn't withhold any tax in AT
                withholding_tax: dec!(0.0),
                withholding_tax_currency: "EUR".to_string(),
            };
            add_trade_to_db(trade, None).await?;
        }
        RecordType::BondTrade => {
            let date_match_regex = r"(..\...\.....(?:,)? um ..:..)";
            let does_date_match_exist = does_match_exist(date_match_regex, text);
            //skip file if it's not a valid trade confirmation
            if !does_date_match_exist {
                return Ok(());
            }
            let date_match = return_first_match(date_match_regex, text)?
                .replace(", um", "")
                .replace(" um", "");

            let date_string_to_parse = format!("{date_match}:00");
            let date = parse_timestamp(&date_string_to_parse)?;
            let isin =
                return_first_match(r"\b[a-zA-Z]{2}\s*[0-9a-zA-Z]{9}[0-9](?![0-9a-zA-Z-])", text)?;

            let avg_price_per_unit = return_first_match(r"(\d+,\d+\s*%)", text)?
                .replace(" %", "")
                .replace(',', ".")
                .parse::<Decimal>()?;

            let units = return_first_match(
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

            let id = return_first_match(r"AUSFUHRUNG\s*(\S+)|Ausfuhrung\s*(\S+))", text)?
                .replace("AUSFUHRUNG", "")
                .replace("AusfUhrung", "")
                .replace(" ", "")
                .replace("\n", "");

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                units,
                direction: direction.to_string(),
                security_type: "Bond".to_string(),
                // TR only supports EUR
                currency: "EUR".to_string(),
                date_added: Utc::now(),
                fees,
                // TR doesn't withhold any tax in AT
                withholding_tax: dec!(0.0),
                withholding_tax_currency: "EUR".to_string(),
            };
            add_with_import_confirmation(trade, id).await?;
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
                // TR doesn't withhold any tax in AT
                withholding_tax: dec!(0.0),
                withholding_tax_currency: "EUR".to_string(),
            };

            add_dividend_to_db(dividend).await?;
        }

        RecordType::EquityTrade => {
            let date_match_regex = r"(..\...\.....(?:,)? um ..:..)";

            let does_date_match_exist = does_match_exist(date_match_regex, text);
            //skip file if it's not a valid trade confirmation
            if !does_date_match_exist {
                return Ok(());
            }
            let date_match = return_first_match(date_match_regex, text)?
                .replace(", um", "")
                .replace(" um", "");

            let date_string_to_parse = format!("{date_match}:00");
            let date = parse_timestamp(&date_string_to_parse)?;

            let isin_match_regex = r"ISIN:\s([A-Z]{2}[0-9A-Z]{10})";

            let does_isin_match_exist = does_match_exist(isin_match_regex, text);
            if !does_isin_match_exist {
                return Ok(());
            }

            let isin = return_first_match(isin_match_regex, text)?.replace("ISIN: ", "");

            let units_match = return_first_match(units_default_regex, text)?;
            let units = units_match
                .replace(" Stk.", "")
                .replace(" EUR", "")
                .replace(" ", "")
                .replace('.', "")
                .replace(',', ".")
                .parse::<Decimal>()?;

            let avg_price_per_unit =
                return_first_match(r"Stk\.\s(\d{1,3}(?:\.\d{3})*,\d{1,3})", text)?
                    .replace(" EUR", "")
                    .replace("Stk.", "")
                    .replace('.', "")
                    .replace(',', ".")
                    .replace(" ", "")
                    .parse::<Decimal>()?;

            let direction = if does_match_exist(r"\sKauf", text)
                || does_match_exist(r"OrderKauf", text)
                || does_match_exist(r"\sSparplan", text)
                || does_match_exist(r"(?i)\bbuy\b", text)
            {
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

            let id = return_first_match(r"(?i)ausfuhrung\s*(\S+)", text)? // `(?i)` makes it case-insensitive
                .split_whitespace() // Removes spaces and newlines
                .last() // Extracts the captured value
                .unwrap_or("")
                .to_uppercase(); // Or `.to_lowercase()`, depending on your needs

            let trade = Trade {
                broker,
                date,
                isin,
                avg_price_per_unit,
                // TR only supports EUR
                eur_avg_price_per_unit: avg_price_per_unit,
                units,
                direction: direction.to_string(),
                security_type: "Equity".to_string(),
                // TR only supports EUR
                currency: "EUR".to_string(),
                date_added: Utc::now(),
                fees,
                // TR doesn't withhold any tax in AT
                withholding_tax: dec!(0.0),
                withholding_tax_currency: "EUR".to_string(),
            };
            add_with_import_confirmation(trade, id).await?;
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
                // TR doesn't withhold any tax in AT
                withholding_tax: dec!(0.0),
                withholding_tax_currency: "EUR".to_string(),
            };

            add_interest_to_db(interest_payment).await?;
        }
        RecordType::PortfolioTransfer => {
            info!("Portfolio transfer, skipping.")
        }
        RecordType::Unmatched => {
            info!("No valid statement found, skipping.")
        }
    }
    Ok(())
}
