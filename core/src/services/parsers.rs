use anyhow::anyhow;
use chrono::prelude::*;
use fancy_regex::Regex;
use std::io::Cursor;

use csv::ReaderBuilder;
use deunicode::deunicode;

use super::importers::{
    erste_bank::extract_erste_bank_record, ibkr::extract_ibkr_record,
    lightyear::extract_lightyear_record, manual::extract_manual_record,
    revolut::extract_revolut_record, scalable::extract_scalable_record,
    trade_republic::extract_trade_republic_record, trading212::extract_trading212_record,
    wise::extract_wise_record,
};

#[derive(Debug)]
pub enum FileFormat {
    Pdf,
    Csv,
    Unsupported,
}

#[derive(Debug)]
pub enum Broker {
    TradeRepublic,
    Revolut,
    Lightyear,
    InteractiveBrokers,
    Scalable,
    ErsteBank,
    Trading212,
    Wise,
    Manual,
}

pub fn detect_broker_from_csv_header(record: &csv::StringRecord) -> anyhow::Result<Option<Broker>> {
    if record.get(0).unwrap().contains("Action") {
        return Ok(Some(Broker::Trading212));
    }
    if record.get(0).unwrap() == "Traded Asset ID Type"
        || record.get(0).unwrap() == "TransferWise ID"
    {
        return Ok(Some(Broker::Wise));
    }
    if (record.get(0).unwrap() == "Date" && record.get(1).unwrap() == "Ticker")
        || record.get(1).unwrap() == "Product"
    {
        return Ok(Some(Broker::Revolut));
    }
    if record.get(0).unwrap() == "Date" && record.get(1).unwrap() == "Reference" {
        return Ok(Some(Broker::Lightyear));
    }
    if record.get(0).unwrap() == "ClientAccountID" {
        return Ok(Some(Broker::InteractiveBrokers));
    }
    if record.get(0).unwrap() == "date" && record.get(4).unwrap() == "direction" {
        return Ok(Some(Broker::Manual));
    }
    Ok(None)
}

pub fn detect_broker_from_pdf_text(text: &str) -> Option<Broker> {
    if does_match_exist(r"TRADE REPUBLIC BANK GMBH", text) {
        return Some(Broker::TradeRepublic);
    }
    if does_match_exist(r"Erste Bank", text) {
        return Some(Broker::ErsteBank);
    }
    if does_match_exist(r"Scalable", text) {
        return Some(Broker::Scalable);
    }
    None
}

pub fn detect_file_format(file: &[u8]) -> FileFormat {
    if file.is_empty() {
        return FileFormat::Unsupported; // No file content
    }

    if file.starts_with(b"%PDF") {
        if let Some(pos) = file.windows(5).position(|window| window == b"%%EOF") {
            if pos > file.len() - 1000 {
                return FileFormat::Pdf;
            }
        }
    }

    if file.contains(&b',' as &u8)
        && (file.contains(&b'\n' as &u8) || file.contains(&b'\r' as &u8))
        && file.windows(5).any(|window| window == b"data,")
    {
        return FileFormat::Csv;
    }

    FileFormat::Unsupported
}

pub fn extract_pdf_text(file: &[u8]) -> anyhow::Result<String> {
    let text = pdf_extract::extract_text_from_mem(file)?;
    let re = Regex::new(r"\s+").unwrap();
    let cleaned_text = deunicode(&text)
        .replace('\0', "")
        .replace("[?]", "")
        .replace("\n", " ")
        .replace("\r", " ")
        .trim()
        .to_string();

    Ok(re.replace_all(&cleaned_text, " ").to_string())
}

pub async fn parse_file_for_import(file: &[u8]) -> anyhow::Result<()> {
    let file_format = detect_file_format(file);

    match file_format {
        FileFormat::Pdf => {
            let text = extract_pdf_text(file)?;

            let broker = detect_broker_from_pdf_text(&text);

            match broker {
                Some(Broker::TradeRepublic) => {
                    extract_trade_republic_record(&text).await?;
                }
                Some(Broker::Scalable) => {
                    extract_scalable_record(&text).await?;
                }
                Some(Broker::ErsteBank) => {
                    extract_erste_bank_record(&text).await?;
                }
                Some(_) => panic!("Broker wrongly matched"),
                None => println!("No broker matched"),
            }
        }
        FileFormat::Csv => {
            let file_content = file;

            let cursor = Cursor::new(file_content);

            let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(cursor);

            let broker = detect_broker_from_csv_header(rdr.headers()?)?;

            match broker {
                Some(Broker::Trading212) => {
                    extract_trading212_record(file_content).await?;
                }
                Some(Broker::Revolut) => {
                    extract_revolut_record(file_content).await?;
                }
                Some(Broker::Lightyear) => {
                    extract_lightyear_record(file_content).await?;
                }
                Some(Broker::InteractiveBrokers) => {
                    extract_ibkr_record(file_content).await?;
                }
                Some(Broker::Wise) => {
                    extract_wise_record(file_content).await?;
                }
                Some(Broker::Manual) => {
                    extract_manual_record(file_content).await?;
                }
                Some(_) => panic!("Broker wrongly matched"),
                None => println!("No broker matched"),
            }
        }
        FileFormat::Unsupported => println!("File unsupported, skipping"),
    }
    Ok(())
}

pub fn does_match_exist(regex_pattern: &str, text: &str) -> bool {
    let regex = Regex::new(regex_pattern).unwrap();
    regex.is_match(text).unwrap()
}

pub fn return_first_match(regex_pattern: &str, text: &str) -> anyhow::Result<String> {
    let regex = Regex::new(regex_pattern)?;
    let caps = regex
        .captures(text)?
        .unwrap_or_else(|| panic!("Expected regex {} couldn't be found", regex_pattern));
    let matched_text = caps.get(0).unwrap();
    Ok(matched_text.as_str().to_string())
}

pub fn remove_first_and_last(value: &str) -> &str {
    let mut chars = value.chars();
    chars.next();
    chars.next_back();
    chars.as_str()
}

pub fn parse_timestamp(timestamp_str: &str) -> anyhow::Result<DateTime<Utc>> {
    let formats = [
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.3fZ",
        "%Y-%m-%dT%H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%.f%#z",
        "%d.%m.%Y %H:%M:%S",
        "%Y%m%d;%H%M%S",
        "%d/%m/%Y %H:%M:%S",
        "%d.%m.%Y %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.3f",
        "%d.%m.%Y %H:%M:%S:%f",
    ];
    for format in formats.iter() {
        if let Ok(timestamp) = NaiveDateTime::parse_from_str(timestamp_str, format) {
            return Ok(timestamp.and_utc());
        }
    }
    Err(anyhow!("Unable to parse timestamp"))
}
