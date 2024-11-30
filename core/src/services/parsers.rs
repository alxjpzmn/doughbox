use std::io::Cursor;

use crate::util::general_helpers::does_match_exist;
use csv::ReaderBuilder;
use deunicode::deunicode;

use super::importers::{
    erste_bank::extract_erste_bank_record, ibkr::extract_ibkr_record,
    lightyear::extract_lightyear_record, manual::extract_manual_record,
    revolut::extract_revolut_record, scalable::extract_scalable_record,
    trade_republic::extract_trade_republic_record, trading212::extract_trading212_record,
    wise::extract_wise_record,
};

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
    let file_content = file;
    // Check for PDF (starts with '%PDF')
    if file_content.starts_with(b"%PDF") {
        return FileFormat::Pdf;
    }
    // Check for CSV (basic heuristic: check for common CSV delimiters like ',' and line breaks)
    // Look for commas and newlines as basic indicators of CSV
    if file_content.contains(&b',' as &u8)
        && (file_content.contains(&b'\n' as &u8) || file_content.contains(&b'\r' as &u8))
    {
        return FileFormat::Csv;
    }
    FileFormat::Unsupported
}

pub async fn parse_file_for_import(file: &[u8]) -> anyhow::Result<()> {
    let file_format = detect_file_format(file);

    match file_format {
        FileFormat::Pdf => {
            let mut text = pdf_extract::extract_text_from_mem(file)?;
            text = deunicode(&text).replace('\0', "").replace("[?]", "");

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
