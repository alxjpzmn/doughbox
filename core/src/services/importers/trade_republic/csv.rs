use anyhow::anyhow;
use csv::ReaderBuilder;
use csv::StringRecord;
use log::info;
use rust_decimal_macros::dec;
use std::io::Cursor;

use crate::database::db_client;
use crate::database::models::dividend::Dividend;
use crate::database::models::interest::InterestPayment;
use crate::database::models::tax_optimization::TaxOptimization;
use crate::database::models::trade::Trade;
use crate::database::queries::composite::add_trade_to_db;
use crate::database::queries::dividend::add_dividend_to_db;
use crate::database::queries::interest::add_interest_to_db;
use crate::database::queries::tax_optimization::add_tax_optimization_to_db;
use crate::database::queries::trade::find_similar_trade;
use crate::services::parsers::parse_timestamp;
use crate::services::shared::util::hash_string;
use chrono::prelude::*;
use rust_decimal::Decimal;

// Check if trade with exact same transaction ID already exists
async fn trade_with_transaction_id_exists(id: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    let hash = hash_string(&format!("Trade Republic{}", id));

    let row = client
        .query_opt("SELECT 1 FROM trade WHERE hash = $1", &[&hash])
        .await?;

    Ok(row.is_some())
}

// Non-interactive version for CSV imports - uses exact transaction_id match
async fn add_csv_trade_with_duplicate_check(trade: Trade, id: String) -> anyhow::Result<bool> {
    // Check for exact duplicate by transaction_id
    if trade_with_transaction_id_exists(&id).await? {
        return Ok(false);
    }

    // Also check for similar trade (same ISIN, date, units, price) as extra safety
    if let Some(ref existing) = find_similar_trade(&trade).await? {
        log::debug!(
            "Skipping duplicate trade (similar found - hash: {}): ISIN {} date {}",
            existing.hash,
            trade.isin,
            trade.date
        );
        return Ok(false);
    }

    // No duplicate found, add the trade
    add_trade_to_db(trade, Some(id)).await?;
    Ok(true)
}

// CSV Import Functions

fn find_column_index(
    headers: &StringRecord,
    column_name: &str,
    required: bool,
) -> anyhow::Result<Option<usize>> {
    let idx = headers.iter().position(|h| h == column_name);
    if required {
        idx.ok_or_else(|| anyhow!("Missing required column: {}", column_name))
            .map(Some)
    } else {
        Ok(idx)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CsvRecordType {
    EquityTrade,
    Dividend,
    InterestPayment,
    Liquidation,
    TaxOptimization,
    DividendEquivalent,
    Skip,
    Unmatched,
}

fn detect_csv_record_type(
    csv_type: &str,
    category: &str,
    asset_class: Option<&str>,
    description: Option<&str>,
) -> CsvRecordType {
    // Skip crypto trades - they use ticker symbols instead of proper ISINs
    if asset_class == Some("CRYPTO") {
        return CsvRecordType::Skip;
    }

    match (csv_type, category) {
        // Trading operations
        ("BUY" | "bUY" | "BUY savings", "TRADING") => CsvRecordType::EquityTrade,
        ("SELL" | "sELL", "TRADING") => CsvRecordType::EquityTrade,
        // Cash interest payments
        ("INTEREST_PAYMENT", "CASH") => CsvRecordType::InterestPayment,
        // Dividends and distributions
        ("DIVIDEND" | "DISTRIBUTION", "CASH") => CsvRecordType::Dividend,
        // Corporate actions - redemptions (liquidations)
        ("REDEMPTION", "CORPORATE_ACTION") => CsvRecordType::Liquidation,
        // Tax optimizations
        ("TAX_OPTIMIZATION", "CASH") => CsvRecordType::TaxOptimization,
        // Earnings (taxable income like referral bonuses) - treat as interest
        // EARNINGS/CASH with "dividend equivalent" description are Austrian Dividend Equivalents
        // (fictitious dividend equivalents from accumulating ETFs). TR withholds tax
        // from the cash balance — these are taxes paid, and should be considered in the total taxes
        // already withheld for the year.
        ("EARNINGS", "CASH") => {
            if description.is_some_and(|d| d.to_lowercase().contains("dividend equivalent")) {
                CsvRecordType::DividendEquivalent
            } else {
                CsvRecordType::InterestPayment
            }
        }
        // Skip these types
        (
            "CUSTOMER_INBOUND"
            | "CUSTOMER_OUTBOUND_REQUEST"
            | "CUSTOMER_OUTBOUND"
            | "CUSTOMER_INPAYMENT"
            | "CUSTOMER_INPAYMENT_REVERSAL",
            "CASH",
        ) => CsvRecordType::Skip,
        ("BONUS" | "COMPENSATION" | "BENEFITS_SAVEBACK", "CASH") => CsvRecordType::Skip,
        ("FREE_RECEIPT" | "FREE_DELIVERY" | "MIGRATION", "DELIVERY") => CsvRecordType::Skip,
        // ADR discontinuation is handled by listing_change table, skip these trades
        // The listing change maps old ISIN to new ISIN, so we don't need duplicate transfer trades
        ("ADR_DISCONTINUATION", "CORPORATE_ACTION") => CsvRecordType::Skip,
        ("CARD_TRANSACTION" | "CARD_TRANSACTION_INTERNATIONAL", "CASH") => CsvRecordType::Skip,
        ("STOCKPERK", "CASH") => CsvRecordType::Skip,
        ("GIFT", "CASH") => CsvRecordType::Skip,
        (
            "TRANSFER_INSTANT_INBOUND"
            | "TRANSFER_INSTANT_OUTBOUND"
            | "TRANSFER_INBOUND"
            | "TRANSFER_OUTBOUND"
            | "VIBAN_TRANSFER_INBOUND",
            "CASH",
        ) => CsvRecordType::Skip,
        ("FEE", "CASH") => CsvRecordType::Skip,
        ("FINAL_MATURITY", "CASH") => CsvRecordType::Skip,
        _ => CsvRecordType::Unmatched,
    }
}

fn parse_csv_decimal(value: &str) -> anyhow::Result<Decimal> {
    if value.is_empty() {
        return Ok(dec!(0));
    }
    value
        .parse::<Decimal>()
        .map_err(|e| anyhow!("Failed to parse decimal '{}': {}", value, e))
}

pub async fn extract_trade_republic_csv_record(file_content: &[u8]) -> anyhow::Result<()> {
    let broker = "Trade Republic".to_string();
    let cursor = Cursor::new(file_content);
    let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(cursor);
    let headers = rdr.headers()?.clone();

    // Find column indices
    let datetime_idx = find_column_index(&headers, "datetime", true)?.unwrap();
    let type_idx = find_column_index(&headers, "type", true)?.unwrap();
    let category_idx = find_column_index(&headers, "category", true)?.unwrap();
    let asset_class_idx = find_column_index(&headers, "asset_class", false)?;
    let isin_idx = find_column_index(&headers, "symbol", true)?.unwrap();
    let shares_idx = find_column_index(&headers, "shares", false)?;
    let price_idx = find_column_index(&headers, "price", false)?;
    let amount_idx = find_column_index(&headers, "amount", true)?.unwrap();
    let fee_idx = find_column_index(&headers, "fee", true)?.unwrap();
    let tax_idx = find_column_index(&headers, "tax", true)?.unwrap();
    let transaction_id_idx = find_column_index(&headers, "transaction_id", true)?.unwrap();
    let name_idx = find_column_index(&headers, "name", false)?;
    let description_idx = find_column_index(&headers, "description", false)?;
    let original_amount_idx = find_column_index(&headers, "original_amount", false)?;
    let original_currency_idx = find_column_index(&headers, "original_currency", false)?;
    let _fx_rate_idx = find_column_index(&headers, "fx_rate", false)?;

    let mut record_count = 0;
    let mut trade_count = 0;
    let mut trade_insert_attempted = 0;
    let mut trade_insert_succeeded = 0;
    let mut trade_skipped_duplicate = 0;
    let mut dividend_count = 0;
    let mut dividend_inserted = 0;
    let mut dividend_duplicates = 0;
    let mut interest_count = 0;
    let mut interest_inserted = 0;
    let mut interest_duplicates = 0;
    let mut tax_opt_count = 0;
    let mut tax_opt_inserted = 0;
    let mut tax_opt_duplicates = 0;
    let mut liquidation_count = 0;
    let mut liquidation_inserted = 0;
    let mut liquidation_duplicates = 0;
    let mut skip_count = 0;
    let mut unmatched_count = 0;

    for result in rdr.records() {
        record_count += 1;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                log::error!("Failed to parse CSV record {}: {:?}", record_count, e);
                continue;
            }
        };

        let csv_type = &record[type_idx];
        let category = &record[category_idx];
        let asset_class = asset_class_idx.and_then(|idx| record.get(idx));
        let description = description_idx.and_then(|idx| record.get(idx));

        let record_type = detect_csv_record_type(csv_type, category, asset_class, description);

        match &record_type {
            CsvRecordType::EquityTrade => trade_count += 1,
            CsvRecordType::Dividend => dividend_count += 1,
            CsvRecordType::InterestPayment => interest_count += 1,
            CsvRecordType::TaxOptimization | CsvRecordType::DividendEquivalent => {
                tax_opt_count += 1
            }
            CsvRecordType::Liquidation => liquidation_count += 1,
            CsvRecordType::Skip => skip_count += 1,
            CsvRecordType::Unmatched => {
                unmatched_count += 1;
                if unmatched_count <= 10 {
                    log::info!(
                        "Unmatched CSV record {}: type={}, category={}",
                        record_count,
                        csv_type,
                        category
                    );
                }
            }
        }

        match record_type {
            CsvRecordType::EquityTrade => {
                let datetime_str = &record[datetime_idx];
                let date = match parse_timestamp(datetime_str) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!(
                            "Failed to parse date '{}' at record {}: {:?}",
                            datetime_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let isin = record[isin_idx].to_string();
                let shares_str = shares_idx.and_then(|idx| record.get(idx)).unwrap_or("");
                let shares = match parse_csv_decimal(shares_str) {
                    Ok(s) => s.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse shares '{}' at record {}: {:?}",
                            shares_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let price_str = price_idx.and_then(|idx| record.get(idx)).unwrap_or("");
                let avg_price_per_unit = match parse_csv_decimal(price_str) {
                    Ok(p) => p.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse price '{}' at record {}: {:?}",
                            price_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let amount_str = &record[amount_idx];
                let amount = match parse_csv_decimal(amount_str) {
                    Ok(a) => a,
                    Err(e) => {
                        log::error!(
                            "Failed to parse amount '{}' at record {}: {:?}",
                            amount_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let fee_str = &record[fee_idx];
                let fee = match parse_csv_decimal(fee_str) {
                    Ok(f) => f.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse fee '{}' at record {}: {:?}",
                            fee_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let tax_str = &record[tax_idx];
                let withholding_tax = match parse_csv_decimal(tax_str) {
                    Ok(t) => t.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse tax '{}' at record {}: {:?}",
                            tax_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let direction = if csv_type == "SELL" || csv_type == "sELL" || amount > dec!(0) {
                    "Sell".to_string()
                } else {
                    "Buy".to_string()
                };

                let security_type = match asset_class {
                    Some("STOCK") => "Equity".to_string(),
                    Some("FUND") => "Equity".to_string(),
                    Some("DERIVATIVE") => "Derivative".to_string(),
                    Some("BOND") => "Bond".to_string(),
                    _ => "Equity".to_string(),
                };

                let transaction_id = record[transaction_id_idx].to_string();

                let trade = Trade {
                    broker: broker.clone(),
                    date,
                    isin,
                    avg_price_per_unit,
                    eur_avg_price_per_unit: avg_price_per_unit,
                    units: shares,
                    direction,
                    security_type,
                    currency: "EUR".to_string(),
                    date_added: Utc::now(),
                    fees: fee,
                    withholding_tax,
                    withholding_tax_currency: "EUR".to_string(),
                };

                trade_insert_attempted += 1;
                match add_csv_trade_with_duplicate_check(trade, transaction_id).await {
                    Ok(true) => {
                        trade_insert_succeeded += 1;
                    }
                    Ok(false) => {
                        trade_skipped_duplicate += 1;
                    }
                    Err(e) => {
                        log::error!("Failed to add trade at record {}: {:?}", record_count, e);
                    }
                }
            }
            CsvRecordType::Dividend => {
                let datetime_str = &record[datetime_idx];
                let date = match parse_timestamp(datetime_str) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!(
                            "Failed to parse date '{}' at record {}: {:?}",
                            datetime_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let isin = record[isin_idx].to_string();

                // For dividends, amount is in the amount column (positive for incoming)
                let amount_str = &record[amount_idx];
                let amount = match parse_csv_decimal(amount_str) {
                    Ok(a) => a.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse amount '{}' at record {}: {:?}",
                            amount_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let tax_str = &record[tax_idx];
                let withholding_tax = match parse_csv_decimal(tax_str) {
                    Ok(t) => t.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse tax '{}' at record {}: {:?}",
                            tax_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                // Check for foreign currency dividend
                let (currency, amount_eur) = if let (Some(orig_amt), Some(orig_curr)) = (
                    original_amount_idx.and_then(|idx| record.get(idx)),
                    original_currency_idx.and_then(|idx| record.get(idx)),
                ) {
                    if !orig_curr.is_empty() && orig_curr != "EUR" {
                        match parse_csv_decimal(orig_amt) {
                            Ok(orig_amount) => (orig_curr.to_string(), orig_amount),
                            Err(e) => {
                                log::error!(
                                    "Failed to parse original amount '{}' at record {}: {:?}",
                                    orig_amt,
                                    record_count,
                                    e
                                );
                                continue;
                            }
                        }
                    } else {
                        ("EUR".to_string(), amount)
                    }
                } else {
                    ("EUR".to_string(), amount)
                };

                let dividend = Dividend {
                    isin: isin.clone(),
                    date,
                    amount: amount_eur,
                    broker: broker.clone(),
                    currency,
                    amount_eur: amount,
                    withholding_tax,
                    withholding_tax_currency: "EUR".to_string(),
                };

                let transaction_id = record[transaction_id_idx].to_string();
                match add_dividend_to_db(dividend, Some(&transaction_id)).await {
                    Ok(true) => {
                        dividend_inserted += 1;
                        println!("💵 Dividend added: ISIN {} on {}", isin, date);
                    }
                    Ok(false) => dividend_duplicates += 1,
                    Err(e) => {
                        log::error!("Failed to add dividend at record {}: {:?}", record_count, e);
                    }
                }
            }
            CsvRecordType::InterestPayment => {
                let datetime_str = &record[datetime_idx];
                let date = match parse_timestamp(datetime_str) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!(
                            "Failed to parse date '{}' at record {}: {:?}",
                            datetime_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let amount_str = &record[amount_idx];
                let amount = match parse_csv_decimal(amount_str) {
                    Ok(a) => a.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse amount '{}' at record {}: {:?}",
                            amount_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let tax_str = &record[tax_idx];
                let withholding_tax = match parse_csv_decimal(tax_str) {
                    Ok(t) => t.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse tax '{}' at record {}: {:?}",
                            tax_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                // Determine principal: bond interest uses "Security", cash interest uses "Cash"
                let principal = if asset_class == Some("BOND") {
                    "Security".to_string()
                } else {
                    "Cash".to_string()
                };

                // Skip zero-amount interest records (likely tax adjustments, not actual interest)
                if amount == dec!(0) {
                    log::info!(
                        "Skipping zero-amount interest at record {}: {}",
                        record_count,
                        principal
                    );
                    skip_count += 1;
                    continue;
                }

                let interest_payment = InterestPayment {
                    date,
                    amount,
                    broker: broker.clone(),
                    principal: principal.clone(),
                    currency: "EUR".to_string(),
                    amount_eur: amount,
                    withholding_tax,
                    withholding_tax_currency: "EUR".to_string(),
                };

                let transaction_id = record[transaction_id_idx].to_string();
                match add_interest_to_db(interest_payment, Some(&transaction_id)).await {
                    Ok(true) => {
                        interest_inserted += 1;
                        println!(
                            "💵 Interest payment added: {} EUR on {} (principal: {})",
                            amount, date, principal
                        );
                    }
                    Ok(false) => interest_duplicates += 1,
                    Err(e) => {
                        log::error!("Failed to add interest at record {}: {:?}", record_count, e);
                    }
                }
            }
            CsvRecordType::Liquidation => {
                // Handle redemption as a sell trade
                let datetime_str = &record[datetime_idx];
                let date = match parse_timestamp(datetime_str) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!(
                            "Failed to parse date '{}' at record {}: {:?}",
                            datetime_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let isin = record[isin_idx].to_string();

                let shares_str = shares_idx.and_then(|idx| record.get(idx)).unwrap_or("");
                let shares = match parse_csv_decimal(shares_str) {
                    Ok(s) => s.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse shares '{}' at record {}: {:?}",
                            shares_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let price_str = price_idx.and_then(|idx| record.get(idx)).unwrap_or("");
                let avg_price_per_unit = match parse_csv_decimal(price_str) {
                    Ok(p) => p.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse price '{}' at record {}: {:?}",
                            price_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let fee_str = &record[fee_idx];
                let fee = match parse_csv_decimal(fee_str) {
                    Ok(f) => f.abs(),
                    Err(e) => {
                        log::error!(
                            "Failed to parse fee '{}' at record {}: {:?}",
                            fee_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let security_type = match asset_class {
                    Some("BOND") => "Bond".to_string(),
                    Some("DERIVATIVE") => "Derivative".to_string(),
                    _ => "Derivative".to_string(),
                };

                let trade = Trade {
                    broker: broker.clone(),
                    date,
                    isin: isin.clone(),
                    avg_price_per_unit,
                    eur_avg_price_per_unit: avg_price_per_unit,
                    units: shares,
                    direction: "Sell".to_string(),
                    security_type,
                    currency: "EUR".to_string(),
                    date_added: Utc::now(),
                    fees: fee,
                    withholding_tax: dec!(0.0),
                    withholding_tax_currency: "EUR".to_string(),
                };

                let transaction_id = record[transaction_id_idx].to_string();
                match add_csv_trade_with_duplicate_check(trade, transaction_id).await {
                    Ok(true) => {
                        liquidation_inserted += 1;
                        println!(
                            "✅ Liquidation trade added: {} {} on {}",
                            shares, isin, date
                        );
                    }
                    Ok(false) => {
                        liquidation_duplicates += 1;
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to add liquidation trade at record {}: {:?}",
                            record_count,
                            e
                        );
                    }
                }
            }
            CsvRecordType::TaxOptimization => {
                let datetime_str = &record[datetime_idx];
                let date = match parse_timestamp(datetime_str) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!(
                            "Failed to parse date '{}' at record {}: {:?}",
                            datetime_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                // Tax optimization amount is stored in the tax column, not the amount column
                // Positive means tax refund, negative means additional tax paid
                let tax_str = &record[tax_idx];
                let amount = match parse_csv_decimal(tax_str) {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!(
                            "Failed to parse tax optimization amount '{}' at record {}: {:?}",
                            tax_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                // Determine tax type from description or name field
                let description = name_idx
                    .and_then(|idx| record.get(idx))
                    .map(|s| s.to_string());

                // Determine tax type based on description
                let tax_type = if let Some(ref desc) = description {
                    let desc_lower = desc.to_lowercase();
                    if desc_lower.contains("dividend") {
                        "Dividend"
                    } else if desc_lower.contains("interest") {
                        "Interest"
                    } else {
                        "CapitalGains"
                    }
                } else {
                    "CapitalGains"
                };

                let transaction_id = record[transaction_id_idx].to_string();

                // Store as tax optimization - positive amount reduces tax, negative increases tax
                let tax_optimization = TaxOptimization {
                    date,
                    broker: broker.clone(),
                    amount,
                    currency: "EUR".to_string(),
                    amount_eur: amount,
                    tax_type: tax_type.to_string(),
                    description,
                    transaction_id: Some(transaction_id.clone()),
                };

                match add_tax_optimization_to_db(tax_optimization).await {
                    Ok(true) => {
                        tax_opt_inserted += 1;
                        println!(
                            "📝 Tax optimization added: {} EUR (type: {}) on {} - ID: {}",
                            amount, tax_type, date, transaction_id
                        );
                    }
                    Ok(false) => tax_opt_duplicates += 1,
                    Err(e) => {
                        log::error!(
                            "Failed to add tax optimization at record {}: {:?}",
                            record_count,
                            e
                        );
                    }
                }
            }
            CsvRecordType::DividendEquivalent => {
                let datetime_str = &record[datetime_idx];
                let date = match parse_timestamp(datetime_str) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!(
                            "Failed to parse date '{}' at record {}: {:?}",
                            datetime_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                // Dividend equivalent records have amount=0 and the tax withheld in the tax column.
                // Negative tax means tax was withheld from the cash balance.
                let tax_str = &record[tax_idx];
                let amount = match parse_csv_decimal(tax_str) {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!(
                            "Failed to parse tax '{}' at record {}: {:?}",
                            tax_str,
                            record_count,
                            e
                        );
                        continue;
                    }
                };

                let description = description_idx
                    .and_then(|idx| record.get(idx))
                    .map(|s| s.to_string());
                let transaction_id = record[transaction_id_idx].to_string();

                let tax_optimization = TaxOptimization {
                    date,
                    broker: broker.clone(),
                    amount,
                    currency: "EUR".to_string(),
                    amount_eur: amount,
                    tax_type: "Dividend".to_string(),
                    description,
                    transaction_id: Some(transaction_id.clone()),
                };

                match add_tax_optimization_to_db(tax_optimization).await {
                    Ok(true) => {
                        tax_opt_inserted += 1;
                        println!(
                            "📝 Dividend equivalent added: {} EUR (type: Dividend) on {} - ID: {}",
                            amount, date, transaction_id
                        );
                    }
                    Ok(false) => tax_opt_duplicates += 1,
                    Err(e) => {
                        log::error!(
                            "Failed to add dividend equivalent at record {}: {:?}",
                            record_count,
                            e
                        );
                    }
                }
            }
            CsvRecordType::Skip => {
                // Skip cash transfers, bonuses, compensations, etc.
            }
            CsvRecordType::Unmatched => {}
        }
    }

    info!(
        "CSV Import Summary: {} total records processed - \
        Trades: {} (attempted: {}, succeeded: {}, duplicates: {}), \
        Liquidations: {} (succeeded: {}, duplicates: {}), \
        Dividends: {} (succeeded: {}, duplicates: {}), \
        Interest: {} (succeeded: {}, duplicates: {}), \
        Tax Opt: {} (succeeded: {}, duplicates: {}), \
        Skipped: {}, Unmatched: {}",
        record_count,
        trade_count,
        trade_insert_attempted,
        trade_insert_succeeded,
        trade_skipped_duplicate,
        liquidation_count,
        liquidation_inserted,
        liquidation_duplicates,
        dividend_count,
        dividend_inserted,
        dividend_duplicates,
        interest_count,
        interest_inserted,
        interest_duplicates,
        tax_opt_count,
        tax_opt_inserted,
        tax_opt_duplicates,
        skip_count,
        unmatched_count
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_decimal_handles_empty_and_values() {
        assert_eq!(parse_csv_decimal("").unwrap(), dec!(0));
        assert_eq!(parse_csv_decimal("12.50").unwrap(), dec!(12.50));
        assert!(parse_csv_decimal("nope").is_err());
    }

    #[test]
    fn detect_csv_record_types() {
        assert_eq!(
            detect_csv_record_type("BUY", "TRADING", None, None),
            CsvRecordType::EquityTrade
        );
        assert_eq!(
            detect_csv_record_type("SELL", "TRADING", None, None),
            CsvRecordType::EquityTrade
        );
        assert_eq!(
            detect_csv_record_type("INTEREST_PAYMENT", "CASH", None, None),
            CsvRecordType::InterestPayment
        );
        assert_eq!(
            detect_csv_record_type("DIVIDEND", "CASH", None, None),
            CsvRecordType::Dividend
        );
        assert_eq!(
            detect_csv_record_type("TAX_OPTIMIZATION", "CASH", None, None),
            CsvRecordType::TaxOptimization
        );
        assert_eq!(
            detect_csv_record_type(
                "EARNINGS",
                "CASH",
                None,
                Some("Austrian dividend equivalent")
            ),
            CsvRecordType::DividendEquivalent
        );
        assert_eq!(
            detect_csv_record_type("EARNINGS", "CASH", None, Some("Referral bonus")),
            CsvRecordType::InterestPayment
        );
        assert_eq!(
            detect_csv_record_type("BUY", "TRADING", Some("CRYPTO"), None),
            CsvRecordType::Skip
        );
        assert_eq!(
            detect_csv_record_type("CUSTOMER_INBOUND", "CASH", None, None),
            CsvRecordType::Skip
        );
        assert_eq!(
            detect_csv_record_type("UNKNOWN", "CASH", None, None),
            CsvRecordType::Unmatched
        );
        assert_eq!(
            detect_csv_record_type("BUY savings", "TRADING", None, None),
            CsvRecordType::EquityTrade
        );
        assert_eq!(
            detect_csv_record_type("DISTRIBUTION", "CASH", None, None),
            CsvRecordType::Dividend
        );
        assert_eq!(
            detect_csv_record_type("REDEMPTION", "CORPORATE_ACTION", None, None),
            CsvRecordType::Liquidation
        );
        assert_eq!(
            detect_csv_record_type("FEE", "CASH", None, None),
            CsvRecordType::Skip
        );
    }
}
