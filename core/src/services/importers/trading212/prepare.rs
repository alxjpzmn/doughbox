use std::{collections::BTreeMap, io::Cursor};

use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use csv::{ReaderBuilder, StringRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    database::models::{
        auth::UserId, dividend::Dividend, fx_conversion::FxConversion,
        interest::InterestPayment, trade::Trade,
    },
    services::{market_data::fx_rates::convert_amount, parsers::parse_timestamp},
};

#[derive(Clone, Debug)]
pub(crate) struct PreparedT212Batch {
    pub rows: Vec<PreparedT212Row>,
    pub content_hash: String,
    pub skipped_actions: BTreeMap<String, i32>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedT212Row {
    pub row_number: i32,
    pub raw_action: String,
    pub raw_id: Option<String>,
    pub original_timestamp: String,
    pub fingerprint: String,
    pub occurrence: i32,
    pub native_payload: Value,
    pub financial: T212Financial,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum T212Financial {
    Trade(Trade),
    Dividend(Dividend),
    Interest(InterestPayment),
    Fx(FxConversion),
}

impl T212Financial {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Trade(_) => "trade",
            Self::Dividend(_) => "dividend",
            Self::Interest(_) => "interest",
            Self::Fx(_) => "fx",
        }
    }

    pub(crate) fn date(&self) -> DateTime<Utc> {
        match self {
            Self::Trade(value) => value.date,
            Self::Dividend(value) => value.date,
            Self::Interest(value) => value.date,
            Self::Fx(value) => value.date,
        }
    }
}

fn truncate_timestamp(date: DateTime<Utc>) -> DateTime<Utc> {
    // Rounding would move an execution into the next timestamp bucket.
    date.with_nanosecond(date.nanosecond() / 1_000 * 1_000)
        .expect("truncated nanoseconds remain valid")
}

pub(crate) fn canonical_timestamp(date: DateTime<Utc>) -> String {
    truncate_timestamp(date).to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub(crate) fn canonical_decimal(value: Decimal) -> String {
    if value.is_zero() {
        "0".to_owned()
    } else {
        value.normalize().to_string()
    }
}

/// Identity deliberately excludes fees, withholding, derived EUR values and source IDs.
pub(crate) fn fingerprint_native(payload: &Value) -> anyhow::Result<String> {
    if payload.get("version").and_then(Value::as_u64) != Some(1) {
        bail!("Unsupported Trading 212 native payload version");
    }
    let text = |key: &str| -> anyhow::Result<&str> {
        payload.get(key).and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Missing Trading 212 native field: {key}"))
    };
    let decimal = |key: &str| -> anyhow::Result<String> {
        Ok(canonical_decimal(text(key)?.parse::<Decimal>()?))
    };
    let kind = text("kind")?;
    let timestamp = canonical_timestamp(parse_timestamp(text("timestamp")?)?);
    let mut identity = vec![json!("t212-event-v1"), json!(kind), json!(timestamp)];
    match kind {
        "trade" => identity.extend([
            json!(text("isin")?), json!(text("direction")?), json!(decimal("units")?),
            json!(decimal("unit_price")?), json!(text("currency")?),
        ]),
        "dividend" => identity.extend([
            json!(text("isin")?), json!(decimal("amount")?), json!(text("currency")?),
        ]),
        "interest" => identity.extend([
            json!(text("principal")?), json!(decimal("amount")?), json!(text("currency")?),
        ]),
        "fx" => identity.extend([
            json!(decimal("from_amount")?), json!(decimal("to_amount")?),
            json!(text("from_currency")?), json!(text("to_currency")?),
        ]),
        _ => bail!("Unsupported Trading 212 native event kind"),
    }
    Ok(blake3::hash(&serde_json::to_vec(&identity)?).to_hex().to_string())
}

struct Columns {
    action: usize,
    total: usize,
    shares: usize,
    price: usize,
    id: usize,
    isin: usize,
    total_currency: usize,
    price_currency: usize,
    timestamp: usize,
    fees: usize,
    fees_currency: Option<usize>,
    exchange_rate: usize,
    withholding: Option<usize>,
    withholding_currency: Option<usize>,
    notes: Option<usize>,
    from_amount: Option<usize>,
    to_amount: Option<usize>,
    from_currency: Option<usize>,
    to_currency: Option<usize>,
}

impl Columns {
    fn new(headers: &StringRecord) -> anyhow::Result<Self> {
        let optional = |name: &str| headers.iter().position(|header| header == name);
        let required = |name: &str| {
            optional(name).ok_or_else(|| anyhow!("Missing required column: {name}"))
        };
        Ok(Self {
            action: required("Action")?,
            total: required("Total")?,
            shares: required("No. of shares")?,
            price: required("Price / share")?,
            id: required("ID")?,
            isin: required("ISIN")?,
            total_currency: required("Currency (Total)")?,
            price_currency: required("Currency (Price / share)")?,
            timestamp: optional("Time").or_else(|| optional("Time (UTC)"))
                .ok_or_else(|| anyhow!("Missing required column: Time or Time (UTC)"))?,
            fees: required("Currency conversion fee")?,
            fees_currency: optional("Currency (Currency conversion fee)"),
            exchange_rate: required("Exchange rate")?,
            withholding: optional("Withholding tax"),
            withholding_currency: optional("Currency (Withholding tax)"),
            notes: optional("Notes"),
            from_amount: optional("Currency conversion from amount"),
            to_amount: optional("Currency conversion to amount"),
            from_currency: optional("Currency (Currency conversion from amount)"),
            to_currency: optional("Currency (Currency conversion to amount)"),
        })
    }
}

fn optional_field(record: &StringRecord, column: Option<usize>) -> Option<&str> {
    column.and_then(|column| record.get(column)).filter(|value| !value.is_empty())
}

fn required_text<'a>(record: &'a StringRecord, column: usize, name: &str) -> anyhow::Result<&'a str> {
    let value = record.get(column).ok_or_else(|| anyhow!("Missing {name}"))?;
    if value.trim().is_empty() {
        bail!("Missing {name}");
    }
    Ok(value)
}

fn required_decimal(record: &StringRecord, column: usize, name: &str) -> anyhow::Result<Decimal> {
    required_text(record, column, name)?.parse().with_context(|| format!("Invalid {name}"))
}

fn valid_currency(value: &str) -> anyhow::Result<&str> {
    if value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_uppercase()) {
        bail!("Invalid Trading 212 currency");
    }
    Ok(value)
}

fn required_currency<'a>(record: &'a StringRecord, column: usize, name: &str) -> anyhow::Result<&'a str> {
    valid_currency(required_text(record, column, name)?)
}

// These fields were optional in the existing importer, including invalid/blank values.
fn optional_decimal(record: &StringRecord, column: Option<usize>) -> Decimal {
    optional_field(record, column).and_then(|value| value.parse().ok()).unwrap_or(Decimal::ZERO)
}

struct Evidence {
    fees: Decimal,
    fees_currency: String,
    withholding: Decimal,
    withholding_currency: String,
}

enum NativeFinancial {
    Trade {
        isin: String, direction: String, units: Decimal, unit_price: Decimal,
        currency: String, total_currency: String, exchange_rate: Decimal,
    },
    Dividend { isin: String, amount: Decimal, currency: String },
    Interest { principal: &'static str, amount: Decimal, currency: String },
    Fx { from_amount: Decimal, to_amount: Decimal, from_currency: String, to_currency: String },
}

struct PendingRow {
    row_number: i32,
    raw_action: String,
    raw_id: Option<String>,
    original_timestamp: String,
    date: DateTime<Utc>,
    native: NativeFinancial,
    evidence: Evidence,
    native_payload: Value,
    fingerprint: String,
}

fn parse_fx_notes(notes: &str) -> anyhow::Result<(Decimal, String, Decimal, String)> {
    let (from, to) = notes.split_once("->").ok_or_else(|| anyhow!("Invalid FX conversion notes"))?;
    let side = |value: &str| -> anyhow::Result<(Decimal, String)> {
        let mut parts = value.split_whitespace();
        let amount = parts.next().ok_or_else(|| anyhow!("Missing FX amount"))?.parse()?;
        let currency = valid_currency(parts.next().ok_or_else(|| anyhow!("Missing FX currency"))?)?;
        if parts.next().is_some() {
            bail!("Invalid FX conversion notes");
        }
        Ok((amount, currency.to_owned()))
    };
    let (from_amount, from_currency) = side(from)?;
    let (to_amount, to_currency) = side(to)?;
    Ok((from_amount, from_currency, to_amount, to_currency))
}

fn parse_supported_row(record: &StringRecord, columns: &Columns, row_number: i32) -> anyhow::Result<Option<PendingRow>> {
    let action = &record[columns.action];
    // Keep the existing case-sensitive action classification and precedence.
    let kind = if action.contains("Dividend") {
        "dividend"
    } else if action == "Withdrawal" || action == "Deposit" {
        return Ok(None);
    } else if action == "Interest on cash" || action == "Lending interest" {
        "interest"
    } else if action.contains("buy") || action.contains("sell") {
        "trade"
    } else if action == "Currency conversion" {
        "fx"
    } else {
        return Ok(None);
    };
    let original_timestamp = required_text(record, columns.timestamp, "timestamp")?.to_owned();
    let date = truncate_timestamp(parse_timestamp(&original_timestamp)?);
    let mut payload = json!({"version": 1, "kind": kind, "timestamp": canonical_timestamp(date)});
    let mut evidence = Evidence {
        fees: optional_decimal(record, Some(columns.fees)),
        fees_currency: String::new(),
        withholding: optional_decimal(record, columns.withholding),
        withholding_currency: String::new(),
    };
    let native = match kind {
        "trade" => {
            let isin = required_text(record, columns.isin, "ISIN")?.to_owned();
            let direction = if action.contains("buy") { "Buy" } else { "Sell" }.to_owned();
            let units = required_decimal(record, columns.shares, "No. of shares")?;
            let unit_price = required_decimal(record, columns.price, "Price / share")?;
            let currency = required_currency(record, columns.price_currency, "price currency")?.to_owned();
            let total_currency = required_currency(record, columns.total_currency, "total currency")?.to_owned();
            let exchange_rate = required_decimal(record, columns.exchange_rate, "Exchange rate")?;
            if exchange_rate <= Decimal::ZERO {
                bail!("Exchange rate must be positive");
            }
            // Fail an unrepresentable normalization before any enrichment requests.
            unit_price.checked_div(exchange_rate).ok_or_else(|| anyhow!("Invalid trade exchange rate"))?;
            evidence.fees_currency = optional_field(record, columns.fees_currency)
                .unwrap_or(&total_currency).to_owned();
            valid_currency(&evidence.fees_currency)?;
            evidence.withholding_currency = optional_field(record, columns.withholding_currency)
                .unwrap_or(&total_currency).to_owned();
            payload["isin"] = json!(isin);
            payload["direction"] = json!(direction);
            payload["units"] = json!(canonical_decimal(units));
            payload["unit_price"] = json!(canonical_decimal(unit_price));
            payload["currency"] = json!(currency);
            NativeFinancial::Trade { isin, direction, units, unit_price, currency, total_currency, exchange_rate }
        }
        "dividend" => {
            let isin = required_text(record, columns.isin, "ISIN")?.to_owned();
            let amount = required_decimal(record, columns.shares, "No. of shares")?
                .checked_mul(required_decimal(record, columns.price, "Price / share")?)
                .ok_or_else(|| anyhow!("Dividend amount exceeds decimal precision"))?;
            let currency = required_currency(record, columns.price_currency, "price currency")?.to_owned();
            evidence.withholding_currency = optional_field(record, columns.withholding_currency)
                .unwrap_or(&currency).to_owned();
            payload["isin"] = json!(isin);
            payload["amount"] = json!(canonical_decimal(amount));
            payload["currency"] = json!(currency);
            NativeFinancial::Dividend { isin, amount, currency }
        }
        "interest" => {
            let principal = if action == "Interest on cash" { "Cash" } else { "Shares" };
            let amount = required_decimal(record, columns.total, "Total")?;
            let currency = required_currency(record, columns.total_currency, "total currency")?.to_owned();
            evidence.withholding_currency = optional_field(record, columns.withholding_currency)
                .unwrap_or(&currency).to_owned();
            payload["principal"] = json!(principal);
            payload["amount"] = json!(canonical_decimal(amount));
            payload["currency"] = json!(currency);
            NativeFinancial::Interest { principal, amount, currency }
        }
        "fx" => {
            let named = |column, name| -> anyhow::Result<Option<Decimal>> {
                optional_field(record, column).map(|value| value.parse()
                    .with_context(|| format!("Invalid {name}"))).transpose()
            };
            let named_from = named(columns.from_amount, "FX from amount")?;
            let named_to = named(columns.to_amount, "FX to amount")?;
            let named_from_currency = optional_field(record, columns.from_currency);
            let named_to_currency = optional_field(record, columns.to_currency);
            // Notes are a fallback only; complete named fields do not depend on free text.
            let needs_notes = named_from.is_none() || named_to.is_none()
                || named_from_currency.is_none() || named_to_currency.is_none();
            let notes = if needs_notes {
                optional_field(record, columns.notes).map(parse_fx_notes).transpose()?
            } else {
                None
            };
            let from_amount = named_from.or_else(|| notes.as_ref().map(|value| value.0))
                .ok_or_else(|| anyhow!("Missing FX conversion from amount"))?;
            let to_amount = named_to.or_else(|| notes.as_ref().map(|value| value.2))
                .ok_or_else(|| anyhow!("Missing FX conversion to amount"))?;
            let from_currency = named_from_currency.or_else(|| notes.as_ref().map(|value| value.1.as_str()))
                .ok_or_else(|| anyhow!("Missing FX conversion from currency"))?;
            let to_currency = named_to_currency.or_else(|| notes.as_ref().map(|value| value.3.as_str()))
                .ok_or_else(|| anyhow!("Missing FX conversion to currency"))?;
            valid_currency(from_currency)?;
            valid_currency(to_currency)?;
            if from_amount <= Decimal::ZERO || to_amount <= Decimal::ZERO {
                bail!("FX conversion amounts must be positive");
            }
            evidence.fees_currency = optional_field(record, columns.fees_currency).unwrap_or("EUR").to_owned();
            valid_currency(&evidence.fees_currency)?;
            payload["from_amount"] = json!(canonical_decimal(from_amount));
            payload["to_amount"] = json!(canonical_decimal(to_amount));
            payload["from_currency"] = json!(from_currency);
            payload["to_currency"] = json!(to_currency);
            NativeFinancial::Fx { from_amount, to_amount, from_currency: from_currency.to_owned(), to_currency: to_currency.to_owned() }
        }
        _ => unreachable!(),
    };
    if kind == "trade" || kind == "fx" {
        payload["fees"] = json!(canonical_decimal(evidence.fees));
        payload["fees_currency"] = json!(evidence.fees_currency);
    }
    if kind != "fx" {
        payload["withholding_tax"] = json!(canonical_decimal(evidence.withholding));
        payload["withholding_tax_currency"] = json!(evidence.withholding_currency);
    }
    let fingerprint = fingerprint_native(&payload)?;
    let raw_id = record.get(columns.id).filter(|id| !id.trim().is_empty()).map(str::to_owned);
    Ok(Some(PendingRow {
        row_number, raw_action: action.to_owned(), raw_id, original_timestamp,
        date, native, evidence, native_payload: payload, fingerprint,
    }))
}

impl PendingRow {
    async fn enrich(self) -> anyhow::Result<PreparedT212Row> {
        let date = self.date;
        let broker = "Trading212".to_owned();
        let evidence = self.evidence;
        let financial = match self.native {
            NativeFinancial::Trade { isin, direction, units, unit_price, currency, total_currency, exchange_rate } => {
                let eur_avg_price_per_unit = convert_amount(
                    unit_price / exchange_rate, &date.date_naive(), &total_currency, "EUR",
                ).await?;
                let fees = convert_amount(evidence.fees, &date.date_naive(), &evidence.fees_currency, "EUR").await?;
                T212Financial::Trade(Trade {
                    broker, date, units, avg_price_per_unit: unit_price, eur_avg_price_per_unit,
                    security_type: "Equity".to_owned(), direction, currency, isin,
                    date_added: Utc::now(), fees, withholding_tax: evidence.withholding,
                    withholding_tax_currency: evidence.withholding_currency,
                })
            }
            NativeFinancial::Dividend { isin, amount, currency } => {
                let amount_eur = convert_amount(amount, &date.date_naive(), &currency, "EUR").await?;
                T212Financial::Dividend(Dividend {
                    isin, date, amount, broker, currency, amount_eur,
                    withholding_tax: evidence.withholding,
                    withholding_tax_currency: evidence.withholding_currency,
                })
            }
            NativeFinancial::Interest { principal, amount, currency } => {
                let amount_eur = convert_amount(amount, &date.date_naive(), &currency, "EUR").await?;
                T212Financial::Interest(InterestPayment {
                    date, amount, broker, principal: principal.to_owned(), currency, amount_eur,
                    withholding_tax: evidence.withholding,
                    withholding_tax_currency: evidence.withholding_currency,
                })
            }
            NativeFinancial::Fx { from_amount, to_amount, from_currency, to_currency } => {
                T212Financial::Fx(FxConversion {
                    date, broker, from_amount, to_amount, from_currency, to_currency,
                    date_added: Utc::now(), fees: -evidence.fees,
                })
            }
        };
        Ok(PreparedT212Row {
            row_number: self.row_number, raw_action: self.raw_action, raw_id: self.raw_id,
            original_timestamp: self.original_timestamp, fingerprint: self.fingerprint,
            occurrence: 0, native_payload: self.native_payload, financial,
        })
    }
}

// csv intentionally tolerates some malformed quoting. Reject it rather than silently
// changing a financial field's contents, while retaining quoted commas/newlines/quotes.
fn validate_csv_quotes(content: &[u8]) -> anyhow::Result<()> {
    enum State { Start, Plain, Quoted, Closed }
    let mut state = State::Start;
    for &byte in content {
        state = match (&state, byte) {
            (State::Start, b'"') => State::Quoted,
            (State::Start | State::Plain, b',' | b'\n' | b'\r') => State::Start,
            (State::Plain, b'"') => bail!("Invalid CSV quoting"),
            (State::Start | State::Plain, _) => State::Plain,
            (State::Quoted, b'"') => State::Closed,
            (State::Quoted, _) => State::Quoted,
            (State::Closed, b'"') => State::Quoted,
            (State::Closed, b',' | b'\n' | b'\r') => State::Start,
            (State::Closed, _) => bail!("Invalid CSV quoting"),
        };
    }
    if matches!(state, State::Quoted) {
        bail!("Unterminated CSV quoted field");
    }
    Ok(())
}

pub(crate) async fn prepare_trading212_csv(
    _user_id: &UserId,
    file_content: &[u8],
) -> anyhow::Result<PreparedT212Batch> {
    if file_content.is_empty() {
        bail!("Empty Trading 212 CSV");
    }
    // The CSV reader accepts a UTF-8 BOM; do not misclassify a quoted first header.
    validate_csv_quotes(file_content.strip_prefix(b"\xef\xbb\xbf").unwrap_or(file_content))?;
    let mut reader = ReaderBuilder::new().has_headers(true).from_reader(Cursor::new(file_content));
    let columns = Columns::new(reader.headers()?)?;
    let mut pending = Vec::new();
    let mut skipped_actions = BTreeMap::new();
    for (ordinal, record) in reader.records().enumerate() {
        let row_number = i32::try_from(ordinal + 1).context("Too many Trading 212 CSV rows")?;
        let record = record.with_context(|| format!("Invalid CSV row {row_number}"))?;
        match parse_supported_row(&record, &columns, row_number)
            .with_context(|| format!("Invalid Trading 212 row {row_number}"))? {
            Some(row) => pending.push(row),
            None => {
                let count = skipped_actions.entry(record[columns.action].to_owned()).or_insert(0i32);
                *count = count.checked_add(1).ok_or_else(|| anyhow!("Too many skipped rows"))?;
            }
        }
    }
    // Finish native validation of the entire report before any ECB enrichment.
    let mut rows = Vec::with_capacity(pending.len());
    for row in pending {
        rows.push(row.enrich().await?);
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&left, &right| {
        let left = &rows[left];
        let right = &rows[right];
        left.financial.date().cmp(&right.financial.date())
            .then_with(|| left.raw_id.cmp(&right.raw_id))
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
            .then_with(|| left.row_number.cmp(&right.row_number))
    });
    let mut previous_timestamp = None;
    let mut occurrence = 0i32;
    for index in order {
        let row = &mut rows[index];
        let timestamp = row.financial.date();
        if previous_timestamp != Some(timestamp) {
            occurrence = 0;
            previous_timestamp = Some(timestamp);
        }
        occurrence = occurrence.checked_add(1).ok_or_else(|| anyhow!("Too many timestamp occurrences"))?;
        row.occurrence = occurrence;
    }
    Ok(PreparedT212Batch {
        rows, content_hash: blake3::hash(file_content).to_hex().to_string(), skipped_actions,
    })
}

