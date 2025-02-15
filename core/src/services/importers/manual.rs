use std::io::Cursor;

use chrono::Utc;
use csv::ReaderBuilder;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;

use crate::{
    database::{models::trade::Trade, queries::composite::add_trade_to_db},
    services::{market_data::fx_rates::convert_amount, parsers::parse_timestamp},
};

#[derive(Debug, Deserialize)]
pub struct ManualRecord {
    date: String,
    isin: String,
    broker: String,
    action: String,
    direction: String,
    units: String,
    avg_price_per_unit: String,
    currency_denomination: String,
    security_type: String,
    fees: String,
    withholding_tax: String,
    withholding_tax_currency: String,
}

enum RecordType {
    EquityTrade,
    Unmatched,
}

fn detect_record_type(record: &ManualRecord) -> RecordType {
    if record.action == "Trade" {
        return RecordType::EquityTrade;
    }
    RecordType::Unmatched
}

pub async fn extract_manual_record(file_content: &[u8]) -> anyhow::Result<()> {
    let cursor = Cursor::new(file_content);

    let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(cursor);

    for result in rdr.deserialize() {
        let record: ManualRecord = result?;

        let record_type = detect_record_type(&record);

        match record_type {
            RecordType::EquityTrade => {
                let trade = Trade {
                    broker: record.broker,
                    date: parse_timestamp(&record.date)?,
                    isin: record.isin,
                    avg_price_per_unit: record.avg_price_per_unit.parse::<Decimal>()?,
                    eur_avg_price_per_unit: if record.currency_denomination == "EUR" {
                        record.avg_price_per_unit.parse::<Decimal>()?
                    } else {
                        convert_amount(
                            record.avg_price_per_unit.parse::<Decimal>()?,
                            &parse_timestamp(&record.date)?.date_naive(),
                            &record.currency_denomination,
                            "EUR",
                        )
                        .await?
                    },
                    no_units: record.units.parse::<Decimal>()?,
                    direction: record.direction,
                    security_type: record.security_type,
                    currency_denomination: record.currency_denomination.to_string(),
                    date_added: Utc::now(),
                    fees: record.fees.parse::<Decimal>().unwrap_or(dec!(0.0)),
                    withholding_tax: record
                        .withholding_tax
                        .parse::<Decimal>()
                        .unwrap_or(dec!(0.0)),
                    witholding_tax_currency: record.withholding_tax_currency,
                };
                add_trade_to_db(trade, None).await?;
            }
            RecordType::Unmatched => continue,
        }
    }
    Ok(())
}
