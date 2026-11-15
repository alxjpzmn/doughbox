use anyhow::{anyhow, ensure, Context};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};

const HEADERS: [&str; 16] = [
    "Action", "Time", "ISIN", "No. of shares", "Price / share", "Currency (Price / share)",
    "Total", "Currency (Total)", "ID", "Currency conversion fee", "Exchange rate",
    "Withholding tax", "Currency (Withholding tax)", "Currency (Currency conversion fee)",
    "Withholding tax reported", "Execution fees",
];

/// Validate the complete page before preparation/application. Out-of-range rows
/// are filtered; malformed timestamps must never be mistaken for complete coverage.
pub(super) fn history_csv(feed: &str, items: &[Value], from: DateTime<Utc>, to: DateTime<Utc>, instruments: &Value) -> anyhow::Result<String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(HEADERS)?;
    for item in items {
        let row = match feed {
            "orders" => order_row(item, from, to, instruments)?,
            "dividends" => dividend_row(item, from, to)?,
            "transactions" => transaction_row(item, from, to)?,
            _ => anyhow::bail!("Unknown Trading 212 history feed"),
        };
        if let Some(row) = row { writer.write_record(row)?; }
    }
    let bytes = writer.into_inner().map_err(|_| anyhow!("Could not encode Trading 212 activity"))?;
    String::from_utf8(bytes).context("Could not encode Trading 212 activity")
}

type HistoryRow = [String; HEADERS.len()];

fn order_row(item: &Value, from: DateTime<Utc>, to: DateTime<Utc>, instruments: &Value) -> anyhow::Result<Option<HistoryRow>> {
    let order = item.get("order").filter(|order| order.is_object()).context("Trading 212 history item has no order")?;
    let Some(fill) = item.get("fill").filter(|fill| !fill.is_null()) else {
        ensure!(!matches!(text(order, "status"), Some("FILLED" | "PARTIALLY_FILLED")), "Trading 212 filled order has no execution");
        return Ok(None);
    };
    let time = timestamp(fill, "filledAt")?;
    if !in_range(time, from, to) { return Ok(None); }
    let time = text(fill, "filledAt").expect("validated timestamp");
    if text(fill, "type") != Some("TRADE") { return Ok(Some(skipped("Unsupported fill", time))); }
    let Some(units) = fill.get("quantity").and_then(decimal) else { return Ok(Some(skipped("Invalid fill", time))); };
    let Some(price) = fill.get("price").and_then(decimal) else { return Ok(Some(skipped("Invalid fill", time))); };
    if units.is_zero() || price <= Decimal::ZERO { return Ok(Some(skipped("Invalid fill", time))); }
    let side = text(order, "side").unwrap_or(if units.is_sign_negative() { "SELL" } else { "BUY" });
    let action = match side { "BUY" => "Market buy", "SELL" => "Market sell", _ => return Ok(Some(skipped("Invalid fill direction", time))) };
    let units = units.abs();
    let ticker = text(order, "ticker").unwrap_or("");
    let isin = order.get("instrument").and_then(|instrument| text(instrument, "isin"))
        .or_else(|| catalog_field(instruments, ticker, "isin"));
    let Some(isin) = isin else { return Ok(Some(skipped("Missing ISIN", time))); };
    let price_currency = order.get("instrument").and_then(|instrument| text(instrument, "currency"))
        .or_else(|| catalog_field(instruments, ticker, "currency")).unwrap_or("");
    let impact = fill.get("walletImpact");
    let total_currency = impact.and_then(|impact| text(impact, "currency")).unwrap_or(price_currency);
    if !currency(price_currency) || !currency(total_currency) { return Ok(Some(skipped("Invalid fill currency", time))); }
    let rate = if price_currency == total_currency { Decimal::ONE } else {
        match impact.and_then(|impact| impact.get("fxRate")).and_then(decimal) {
            Some(rate) if rate > Decimal::ZERO => rate,
            _ => return Ok(Some(skipped("Missing exchange rate", time))),
        }
    };
    // All charges on a fill are execution costs, including stamp duty and
    // transaction taxes. Preserve each currency until dated EUR enrichment.
    let execution_fees = execution_fees(impact.and_then(|impact| impact.get("taxes")))?;
    let total = units.checked_mul(price).context("Trading 212 fill exceeds decimal precision")?;
    Ok(Some([
        action.into(), time.into(), isin.into(), plain(units), plain(price), price_currency.into(),
        plain(total), total_currency.into(), id_text(fill.get("id")), "0".into(), plain(rate),
        "0".into(), total_currency.into(), total_currency.into(), "true".into(), serde_json::to_string(&execution_fees)?,
    ]))
}

fn dividend_row(item: &Value, from: DateTime<Utc>, to: DateTime<Utc>) -> anyhow::Result<Option<HistoryRow>> {
    if !in_range(timestamp(item, "paidOn")?, from, to) { return Ok(None); }
    let time = text(item, "paidOn").expect("validated timestamp");
    let Some(quantity) = item.get("quantity").and_then(decimal) else { return Ok(Some(skipped("Invalid dividend", time))); };
    let Some(per_share) = item.get("grossAmountPerShare").and_then(decimal) else { return Ok(Some(skipped("Invalid dividend", time))); };
    if quantity <= Decimal::ZERO || per_share < Decimal::ZERO { return Ok(Some(skipped("Invalid dividend", time))); }
    let Some(isin) = item.get("instrument").and_then(|instrument| text(instrument, "isin")) else { return Ok(Some(skipped("Missing ISIN", time))); };
    let Some(currency_code) = text(item, "tickerCurrency").or_else(|| item.get("instrument").and_then(|instrument| text(instrument, "currency"))).filter(|value| currency(value)) else {
        return Ok(Some(skipped("Invalid dividend currency", time)));
    };
    let kind = text(item, "type").unwrap_or("ORDINARY");
    let total = quantity.checked_mul(per_share).context("Trading 212 dividend exceeds decimal precision")?;
    // An unreported tax is not a reported zero. Reconciliation may later fill
    // this field from an authoritative statement without creating another income.
    Ok(Some([
        format!("Dividend ({kind})"), time.into(), isin.into(), plain(quantity), plain(per_share), currency_code.into(),
        plain(total), currency_code.into(), text(item, "reference").unwrap_or("").into(), "0".into(), "1".into(),
        String::new(), String::new(), currency_code.into(), "false".into(), String::new(),
    ]))
}

fn transaction_row(item: &Value, from: DateTime<Utc>, to: DateTime<Utc>) -> anyhow::Result<Option<HistoryRow>> {
    if !in_range(timestamp(item, "dateTime")?, from, to) { return Ok(None); }
    let time = text(item, "dateTime").expect("validated timestamp");
    let action = match text(item, "type") {
        Some("INTEREST_ON_FREE_CASH") => "Interest on cash",
        Some("LENDING_INTEREST") => "Lending interest",
        Some("DEPOSIT") => "Deposit",
        Some("WITHDRAW") => "Withdrawal",
        _ => return Ok(Some(skipped("Unsupported transaction", time))),
    };
    let Some(amount) = item.get("amount").and_then(decimal) else { return Ok(Some(skipped("Invalid interest amount", time))); };
    let currency_code = text(item, "currency").unwrap_or("");
    if !currency(currency_code) { return Ok(Some(skipped("Invalid interest currency", time))); }
    // Corrections retain their sign; zero is still a valid reported amount.
    Ok(Some([
        action.into(), time.into(), String::new(), "0".into(), "0".into(), String::new(), plain(amount), currency_code.into(),
        text(item, "reference").unwrap_or("").into(), "0".into(), "1".into(), String::new(), String::new(),
        currency_code.into(), "false".into(), String::new(),
    ]))
}

fn execution_fees(taxes: Option<&Value>) -> anyhow::Result<Vec<Value>> {
    let Some(taxes) = taxes.filter(|taxes| !taxes.is_null()) else { return Ok(Vec::new()); };
    taxes.as_array().context("Trading 212 execution charges are not a list")?.iter().map(|tax| {
        let amount = tax.get("quantity").and_then(decimal).context("Invalid Trading 212 execution charge")?;
        let code = text(tax, "currency").filter(|code| currency(code)).context("Invalid Trading 212 execution charge currency")?;
        Ok(json!({"amount":plain(amount), "currency":code, "name":text(tax,"name").unwrap_or("Execution charge")}))
    }).collect()
}

fn catalog_field<'a>(instruments: &'a Value, ticker: &str, key: &str) -> Option<&'a str> {
    instruments.as_array()?.iter().find(|item| text(item, "ticker") == Some(ticker)).and_then(|item| text(item, key))
}

fn timestamp(item: &Value, field: &str) -> anyhow::Result<DateTime<Utc>> {
    let value = text(item, field).context("Trading 212 history item is missing its timestamp")?;
    DateTime::parse_from_rfc3339(value).map(|date| date.with_timezone(&Utc))
        .map_err(|_| anyhow!("Trading 212 history item has an invalid timestamp"))
}

fn in_range(time: DateTime<Utc>, from: DateTime<Utc>, to: DateTime<Utc>) -> bool { time >= from && time < to }

fn skipped(reason: &str, time: &str) -> HistoryRow {
    let mut row = std::array::from_fn(|_| String::new());
    // Fixed reasons never contain a supported action such as "Dividend" or
    // "Interest on cash"; the CSV importer counts these without financial parsing.
    row[0] = reason.into();
    row[1] = time.into();
    row
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> { value.get(key)?.as_str().filter(|text| !text.is_empty()) }
fn id_text(value: Option<&Value>) -> String {
    match value { Some(Value::String(text)) => text.clone(), Some(Value::Number(number)) => number.to_string(), _ => String::new() }
}
fn decimal(value: &Value) -> Option<Decimal> {
    match value { Value::String(text) => text.parse().ok(), Value::Number(number) => number.to_string().parse().ok(), _ => None }
}
fn plain(value: Decimal) -> String { value.normalize().to_string() }
fn currency(value: &str) -> bool { value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()) }

/// Broker pagination must stay on the current history feed.
pub(super) fn next_history_path(feed: &str, next: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(next) = next.map(str::trim).filter(|next| !next.is_empty() && *next != "null") else { return Ok(None); };
    let path = if let Some(rest) = next.strip_prefix("/api/v0") { rest.to_owned() }
        else if let Some(query) = next.strip_prefix('?') { format!("/equity/history/{feed}?{query}") }
        else if next.starts_with("/equity/history/") { next.to_owned() }
        else { anyhow::bail!("Trading 212 returned a history page link outside the history API"); };
    let base = path.split_once('?').map_or(path.as_str(), |(base, _)| base);
    ensure!(base == format!("/equity/history/{feed}") && !path.contains("//"), "Trading 212 returned a history page link outside the history API");
    Ok(Some(path))
}

pub(super) fn history_feed(phase: &str) -> Option<&'static str> {
    match phase { "history_orders" => Some("orders"), "history_dividends" => Some("dividends"), "history_transactions" => Some("transactions"), _ => None }
}
pub(super) fn next_history_phase(phase: &str) -> Option<&'static str> {
    match phase { "history_orders" => Some("history_dividends"), "history_dividends" => Some("history_transactions"), _ => None }
}
pub(super) const HISTORY_COMPLETE: &str = "history-complete";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::models::auth::UserId,
        services::importers::trading212::{prepare_trading212_csv, T212Financial},
    };
    use rust_decimal_macros::dec;

    fn bounds() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().with_timezone(&Utc),
            DateTime::parse_from_rfc3339("2027-01-01T00:00:00Z").unwrap().with_timezone(&Utc),
        )
    }

    fn fill() -> Value {
        json!({
            "order":{"side":"SELL","instrument":{"isin":"US0378331005","currency":"EUR"}},
            "fill":{"id":42,"filledAt":"2026-11-01T00:00:00Z","type":"TRADE","quantity":2,"price":10,
                "walletImpact":{"currency":"EUR","taxes":[
                    {"name":"CURRENCY_CONVERSION_FEE","currency":"EUR","quantity":0.25},
                    {"name":"STAMP_DUTY","currency":"EUR","quantity":2}
                ]}}
        })
    }

    #[tokio::test]
    async fn execution_charges_are_fees_not_withholding() {
        let (from,to)=bounds();
        let csv=history_csv("orders",&[fill()],from,to,&json!([])).unwrap();
        let owner=UserId::from_database("history-mapping-test".into());
        let batch=prepare_trading212_csv(&owner,csv.as_bytes()).await.unwrap();
        let T212Financial::Trade(trade)=&batch.rows[0].financial else {panic!("expected trade")};
        assert_eq!(trade.fees,dec!(2.25));
        assert_eq!(trade.withholding_tax,Decimal::ZERO);
        assert_eq!(trade.direction,"Sell");
    }

    #[tokio::test]
    async fn signed_and_zero_interest_survive_and_bad_currency_is_skipped() {
        let (from,to)=bounds();
        let rows=[dec!(-1.5),dec!(0),dec!(2)].map(|amount| json!({
            "dateTime":"2026-11-01T00:00:00Z","type":"INTEREST_ON_FREE_CASH",
            "amount":amount.to_string(),"currency":"EUR","reference":amount.to_string()
        }));
        let mut items=rows.to_vec();
        items.push(json!({"dateTime":"2026-11-01T00:00:00Z","type":"LENDING_INTEREST","amount":1,"currency":"Unknown"}));
        let csv=history_csv("transactions",&items,from,to,&json!([])).unwrap();
        let owner=UserId::from_database("history-mapping-test".into());
        let batch=prepare_trading212_csv(&owner,csv.as_bytes()).await.unwrap();
        let amounts=batch.rows.iter().map(|row| {
            let T212Financial::Interest(interest)=&row.financial else {panic!("expected interest")};
            interest.amount
        }).collect::<Vec<_>>();
        assert_eq!(amounts,vec![dec!(-1.5),dec!(0),dec!(2)]);
        assert_eq!(batch.skipped_actions.get("Invalid interest currency"),Some(&1));
        assert!(batch.rows.iter().all(|row| row.native_payload["withholding_tax_reported"]==false));
    }

    #[test]
    fn malformed_timestamps_fail_the_page_but_valid_boundaries_filter() {
        let (from,to)=bounds();
        for field in [Value::Null,json!("invalid"),json!("")] {
            let mut order=fill();
            order["fill"]["filledAt"]=field.clone();
            assert!(history_csv("orders",&[order],from,to,&json!([])).is_err());
            assert!(history_csv("dividends",&[json!({"paidOn":field})],from,to,&json!([])).is_err());
            assert!(history_csv("transactions",&[json!({"dateTime":field})],from,to,&json!([])).is_err());
        }
        let mut order=fill();
        order["fill"]["filledAt"]=json!("2027-01-01T00:00:00Z");
        let csv=history_csv("orders",&[order],from,to,&json!([])).unwrap();
        assert_eq!(csv::Reader::from_reader(csv.as_bytes()).records().count(),0);
    }

    #[tokio::test]
    async fn unreported_income_withholding_is_not_a_reported_zero() {
        let (from,to)=bounds();
        let csv=history_csv("dividends",&[json!({
            "paidOn":"2026-11-01T00:00:00Z","quantity":10,"grossAmountPerShare":1,
            "tickerCurrency":"EUR","instrument":{"isin":"US0378331005"},"reference":"div-1"
        })],from,to,&json!([])).unwrap();
        let owner=UserId::from_database("history-mapping-test".into());
        let batch=prepare_trading212_csv(&owner,csv.as_bytes()).await.unwrap();
        assert_eq!(batch.rows[0].native_payload["withholding_tax_reported"],false);
        let T212Financial::Dividend(dividend)=&batch.rows[0].financial else {panic!("expected dividend")};
        assert_eq!(dividend.amount,dec!(10));
    }
}
