use anyhow::anyhow;
use chrono::prelude::*;
use fancy_regex::Regex;
use log::info;
use std::{io::Cursor, path::Path};

use csv::ReaderBuilder;
use deunicode::deunicode;

use super::{
    files::detect_file_format,
    importers::{
        erste_bank::extract_erste_bank_record,
        ibkr::extract_ibkr_record,
        lightyear::extract_lightyear_record,
        manual::extract_manual_record,
        revolut::extract_revolut_record,
        scalable::extract_scalable_record,
        trade_republic::{extract_trade_republic_csv_record, extract_trade_republic_record},
        trading212::extract_trading212_record,
        wise::extract_wise_record,
    },
};

#[derive(Debug, PartialEq, Eq)]
pub enum ImportFileFormat {
    Pdf,
    Csv,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionMode {
    Interactive,
    NonInteractive,
}

impl InteractionMode {
    pub fn require_interactive(self, reason: &str) -> anyhow::Result<()> {
        if self == Self::NonInteractive {
            return Err(anyhow!("{}; import this statement using the interactive CLI", reason));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
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
    if record.get(0).map(|s| s.contains("Action")).unwrap_or(false)
        && record.iter().any(|s| s == "ISIN")
    {
        return Ok(Some(Broker::Trading212));
    }
    if record.get(0) == Some("Traded Asset ID Type") || record.get(0) == Some("TransferWise ID") {
        return Ok(Some(Broker::Wise));
    }
    if (record.get(0) == Some("Date") && record.get(1) == Some("Ticker"))
        || record.get(1) == Some("Product")
    {
        return Ok(Some(Broker::Revolut));
    }
    if record.get(0) == Some("Date") && record.get(1) == Some("Reference") {
        return Ok(Some(Broker::Lightyear));
    }
    if record.get(0) == Some("ClientAccountID") {
        return Ok(Some(Broker::InteractiveBrokers));
    }
    if record.get(0) == Some("date") && record.get(4) == Some("direction") {
        return Ok(Some(Broker::Manual));
    }
    // Trade Republic CSV has "datetime" as first column and "transaction_id" in column 19 (index 18)
    if record.get(0) == Some("datetime") && record.get(18) == Some("transaction_id") {
        return Ok(Some(Broker::TradeRepublic));
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

pub async fn parse_file_for_import(file: &[u8], file_path: &Path) -> anyhow::Result<()> {
    parse_file(file, file_path, InteractionMode::Interactive).await
}

/// Imports uploaded statements without ever prompting on standard input.
pub async fn parse_file_for_upload(file: &[u8], file_path: &Path) -> anyhow::Result<()> {
    parse_file(file, file_path, InteractionMode::NonInteractive).await
}

async fn parse_file(
    file: &[u8],
    file_path: &Path,
    interaction: InteractionMode,
) -> anyhow::Result<()> {
    // File extensions accepted by uploads are case-insensitive.
    let format_path = match file_path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("pdf") => Path::new("statement.pdf"),
        Some(extension) if extension.eq_ignore_ascii_case("csv") => Path::new("statement.csv"),
        _ => file_path,
    };
    let file_format = detect_file_format(file, format_path);

    match file_format {
        ImportFileFormat::Pdf => {
            let text = extract_pdf_text(file)?;

            let broker = detect_broker_from_pdf_text(&text);
            if interaction == InteractionMode::NonInteractive {
                // Allow the queue to cancel after synchronous extraction if its lock was lost.
                tokio::task::yield_now().await;
            }

            match broker {
                Some(Broker::TradeRepublic) => {
                    info!("Statement from Trade Republic detected.");
                    extract_trade_republic_record(&text, interaction).await?;
                }
                Some(Broker::Scalable) => {
                    info!("Statement from Scalable detected.");
                    extract_scalable_record(&text).await?;
                }
                Some(Broker::ErsteBank) => {
                    info!("Statement from Erste Bank detected.");
                    extract_erste_bank_record(&text).await?;
                }
                Some(_) => return Err(anyhow!("Unsupported PDF broker")),
                None => return Err(anyhow!("No recognized broker found in PDF statement")),
            }
        }
        ImportFileFormat::Csv => {
            let file_content = file;

            let cursor = Cursor::new(file_content);

            let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(cursor);

            let broker = detect_broker_from_csv_header(rdr.headers()?)?;
            if interaction == InteractionMode::NonInteractive {
                tokio::task::yield_now().await;
            }

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
                Some(Broker::TradeRepublic) => {
                    info!("Trade Republic CSV detected.");
                    extract_trade_republic_csv_record(file_content).await?;
                }
                Some(_) => return Err(anyhow!("Unsupported CSV broker")),
                None => return Err(anyhow!("No recognized broker found in CSV header")),
            }
        }
        ImportFileFormat::Unsupported => return Err(anyhow!("Unsupported or empty statement file")),
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
        .ok_or_else(|| anyhow!("Expected statement field not found: {}", regex_pattern))?;
    let matched_text = caps
        .get(0)
        .ok_or_else(|| anyhow!("Expected statement field was empty: {}", regex_pattern))?;
    Ok(matched_text.as_str().to_string())
}

pub fn remove_first_and_last(value: &str) -> &str {
    let mut chars = value.chars();
    chars.next();
    chars.next_back();
    chars.as_str()
}

pub fn parse_timestamp(timestamp_str: &str) -> anyhow::Result<DateTime<Utc>> {
    let tz_formats = [
        "%Y-%m-%d %H:%M:%S%.3f%#z",
        "%Y-%m-%d %H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%#z",
        "%Y-%m-%dT%H:%M:%S%.3f%#z",
        "%Y-%m-%dT%H:%M:%S%.f%#z",
        "%Y-%m-%dT%H:%M:%S%#z",
        "%Y-%m-%dT%H:%M:%S%.3fZ",
        "%Y-%m-%dT%H:%M:%S%.fZ",
        "%Y-%m-%dT%H:%M:%SZ",
    ];
    for format in tz_formats.iter() {
        if let Ok(timestamp) = DateTime::parse_from_str(timestamp_str, format) {
            return Ok(timestamp.with_timezone(&Utc));
        }
    }

    let formats = [
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.3fZ",
        "%Y-%m-%dT%H:%M:%S%.fZ",
        "%d.%m.%Y %H:%M:%S",
        "%Y%m%d;%H%M%S",
        "%d/%m/%Y %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upload_rejects_unsupported_empty_and_unknown_content() {
        for (file, filename) in [
            (b"plain text".as_slice(), "statement.txt"),
            (b"".as_slice(), "statement.csv"),
            (b"not a PDF".as_slice(), "statement.PDF"),
            (b"%PDF-1.7\nmalformed".as_slice(), "statement.PDF"),
            (b"unknown,header\nsome,value\n".as_slice(), "statement.CSV"),
        ] {
            assert!(parse_file_for_upload(file, Path::new(filename)).await.is_err());
        }
    }

    #[tokio::test]
    async fn upload_recognizes_case_insensitive_csv_extension() {
        let error = parse_file_for_upload(b"unknown,header\n", Path::new("statement.CSV"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("No recognized broker"));
    }

    #[tokio::test]
    async fn upload_rejects_other_brokers_unsupported_and_malformed_pdf_records() {
        for text in [
            "Scalable unsupported statement",
            "Scalable Wertpapierabrechnung without required trade fields",
            "Scalable Dividendenabrechnung without required dividend fields",
        ] {
            assert!(extract_scalable_record(text).await.is_err());
        }
        for text in [
            "Erste Bank unsupported statement",
            "Erste Bank Kauf Marktplatz without required trade fields",
        ] {
            assert!(extract_erste_bank_record(text).await.is_err());
        }
        assert_eq!(detect_broker_from_pdf_text("unrecognized statement"), None);
    }

    #[test]
    fn missing_pdf_field_returns_error() {
        assert!(return_first_match(r"\d+", "no amount").is_err());
    }

    #[test]
    fn parse_old_trading212_timestamps() {
        assert_eq!(
            parse_timestamp("2023-12-14 09:43:21").unwrap(),
            NaiveDate::from_ymd_opt(2023, 12, 14)
                .unwrap()
                .and_hms_opt(9, 43, 21)
                .unwrap()
                .and_utc()
        );
        assert_eq!(
            parse_timestamp("2023-12-20 22:10:17.977").unwrap(),
            NaiveDate::from_ymd_opt(2023, 12, 20)
                .unwrap()
                .and_hms_milli_opt(22, 10, 17, 977)
                .unwrap()
                .and_utc()
        );
    }

    #[test]
    fn parse_new_trading212_timestamp() {
        assert_eq!(
            parse_timestamp("2026-08-01 01:03:31+00:00").unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 1)
                .unwrap()
                .and_hms_opt(1, 3, 31)
                .unwrap()
                .and_utc()
        );
    }

    fn header(cols: &[&str]) -> csv::StringRecord {
        csv::StringRecord::from(cols.to_vec())
    }

    #[test]
    fn detect_trading212_csv_header() {
        let record = header(&["Action", "Time", "ISIN", "Ticker"]);
        assert_eq!(
            detect_broker_from_csv_header(&record).unwrap(),
            Some(Broker::Trading212)
        );
    }

    #[test]
    fn detect_wise_csv_header() {
        assert_eq!(
            detect_broker_from_csv_header(&header(&["Traded Asset ID Type"])).unwrap(),
            Some(Broker::Wise)
        );
        assert_eq!(
            detect_broker_from_csv_header(&header(&["TransferWise ID"])).unwrap(),
            Some(Broker::Wise)
        );
    }

    #[test]
    fn detect_revolut_csv_header() {
        assert_eq!(
            detect_broker_from_csv_header(&header(&["Date", "Ticker"])).unwrap(),
            Some(Broker::Revolut)
        );
        assert_eq!(
            detect_broker_from_csv_header(&header(&["Date", "Product"])).unwrap(),
            Some(Broker::Revolut)
        );
    }

    #[test]
    fn detect_lightyear_csv_header() {
        assert_eq!(
            detect_broker_from_csv_header(&header(&["Date", "Reference"])).unwrap(),
            Some(Broker::Lightyear)
        );
    }

    #[test]
    fn detect_ibkr_csv_header() {
        assert_eq!(
            detect_broker_from_csv_header(&header(&["ClientAccountID"])).unwrap(),
            Some(Broker::InteractiveBrokers)
        );
    }

    #[test]
    fn detect_manual_csv_header() {
        assert_eq!(
            detect_broker_from_csv_header(&header(&[
                "date",
                "broker",
                "ISIN",
                "avg_price_per_unit",
                "direction"
            ]))
            .unwrap(),
            Some(Broker::Manual)
        );
    }

    #[test]
    fn detect_trade_republic_csv_header() {
        let mut cols = vec![""; 19];
        cols[0] = "datetime";
        cols[18] = "transaction_id";
        assert_eq!(
            detect_broker_from_csv_header(&header(&cols)).unwrap(),
            Some(Broker::TradeRepublic)
        );
    }

    #[test]
    fn detect_unknown_csv_header() {
        assert_eq!(
            detect_broker_from_csv_header(&header(&["foo", "bar"])).unwrap(),
            None
        );
    }

    #[test]
    fn detect_pdf_brokers() {
        assert_eq!(
            detect_broker_from_pdf_text("TRADE REPUBLIC BANK GMBH Berlin"),
            Some(Broker::TradeRepublic)
        );
        assert_eq!(
            detect_broker_from_pdf_text("Erste Bank der oesterreichischen Sparkassen"),
            Some(Broker::ErsteBank)
        );
        assert_eq!(
            detect_broker_from_pdf_text("Scalable Capital GmbH"),
            Some(Broker::Scalable)
        );
        assert_eq!(detect_broker_from_pdf_text("some other broker"), None);
    }

    #[test]
    fn remove_surrounding_chars() {
        assert_eq!(remove_first_and_last("\"ISIN\""), "ISIN");
    }

    #[test]
    fn parse_additional_timestamp_formats() {
        assert_eq!(
            parse_timestamp("24.12.2023 15:30:00").unwrap(),
            NaiveDate::from_ymd_opt(2023, 12, 24)
                .unwrap()
                .and_hms_opt(15, 30, 0)
                .unwrap()
                .and_utc()
        );
        assert_eq!(
            parse_timestamp("2023-12-24T15:30:00Z").unwrap(),
            NaiveDate::from_ymd_opt(2023, 12, 24)
                .unwrap()
                .and_hms_opt(15, 30, 0)
                .unwrap()
                .and_utc()
        );
        assert!(parse_timestamp("not-a-date").is_err());
    }

    #[test]
    fn match_helpers() {
        assert!(does_match_exist(r"TRADE REPUBLIC", "TRADE REPUBLIC BANK"));
        assert!(!does_match_exist(r"Scalable", "Trade Republic"));
    }
}
