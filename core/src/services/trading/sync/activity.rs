use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

const HEADER: &str = "Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate,Withholding tax,Currency (Withholding tax)";

/// One history-list page, limited to the requested interval, as the CSV the
/// existing importer already reconciles. Rows that cannot be represented are
/// counted as skipped actions rather than written with invented amounts.
pub(super) fn history_csv(feed: &str, items: &[Value], from: DateTime<Utc>, to: DateTime<Utc>, instruments: &Value) -> String {
    let mut csv = String::from(HEADER);
    csv.push('\n');
    for item in items {
        if let Some(row) = row(feed, item, from, to, instruments) {
            csv.push_str(&row);
            csv.push('\n');
        }
    }
    csv
}

fn row(feed: &str, item: &Value, from: DateTime<Utc>, to: DateTime<Utc>, instruments: &Value) -> Option<String> {
    match feed {
        "orders" => order_row(item, from, to, instruments),
        "dividends" => dividend_row(item, from, to),
        "transactions" => transaction_row(item, from, to),
        _ => None,
    }
}

fn order_row(item: &Value, from: DateTime<Utc>, to: DateTime<Utc>, instruments: &Value) -> Option<String> {
    let fill = item.get("fill")?;
    let time = text(fill, "filledAt")?;
    in_range(time, from, to)?;
    let fill_type = text(fill, "type").unwrap_or("UNKNOWN");
    if fill_type != "TRADE" {
        return Some(skipped(fill_type, time));
    }
    let Some(order) = item.get("order") else { return Some(skipped("Missing order", time)); };
    let side = text(order, "side").unwrap_or(if fill.get("quantity").and_then(decimal).is_some_and(|quantity| quantity.is_sign_negative()) { "SELL" } else { "BUY" });
    let action = if side.eq_ignore_ascii_case("sell") { "Market sell" } else { "Market buy" };
    let Some(units) = fill.get("quantity").and_then(decimal).map(|quantity| quantity.abs()) else { return Some(skipped("Invalid fill", time)); };
    let Some(price) = fill.get("price").and_then(decimal) else { return Some(skipped("Invalid fill", time)); };
    if units <= Decimal::ZERO || price <= Decimal::ZERO {
        return Some(skipped("Invalid fill", time));
    }
    let ticker = text(order, "ticker").unwrap_or("");
    let isin = order.get("instrument").and_then(|instrument| text(instrument, "isin")).map(str::to_owned)
        .or_else(|| catalog_field(instruments, ticker, "isin"));
    let Some(isin) = isin.filter(|isin| !isin.is_empty()) else { return Some(skipped("Missing ISIN", time)); };
    let price_currency = order.get("instrument").and_then(|instrument| text(instrument, "currency")).map(str::to_owned)
        .or_else(|| catalog_field(instruments, ticker, "currency")).unwrap_or_default();
    let impact = fill.get("walletImpact");
    let total_currency = impact.and_then(|impact| text(impact, "currency")).unwrap_or(price_currency.as_str()).to_owned();
    if !currency(&price_currency) || !currency(&total_currency) {
        return Some(skipped("Invalid fill currency", time));
    }
    let rate = if price_currency == total_currency {
        Decimal::ONE
    } else {
        match impact.and_then(|impact| impact.get("fxRate")).and_then(decimal) {
            Some(rate) if rate > Decimal::ZERO => rate,
            _ => return Some(skipped("Missing exchange rate", time)),
        }
    };
    let taxes = impact.and_then(|impact| impact.get("taxes")).and_then(Value::as_array);
    let Some(fees) = tax_sum(taxes, "CURRENCY_CONVERSION_FEE", true) else {
        return Some(skipped("Unrepresentable fill taxes", time));
    };
    let Some(withholding) = tax_sum(taxes, "CURRENCY_CONVERSION_FEE", false) else {
        return Some(skipped("Unrepresentable fill taxes", time));
    };
    let (fees, fees_currency) = fees.unwrap_or((Decimal::ZERO, total_currency.clone()));
    let (withholding, withholding_currency) = withholding.unwrap_or((Decimal::ZERO, total_currency.clone()));
    if !currency(&fees_currency) || !currency(&withholding_currency) {
        return Some(skipped("Invalid fill tax currency", time));
    }
    Some(fields(&[
        action, time, &isin, &plain(units), &plain(price), &price_currency, &plain(units * price), &total_currency,
        &id_text(fill.get("id")), &plain(fees), &plain(rate), &plain(withholding), &withholding_currency,
    ]))
}

fn dividend_row(item: &Value, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<String> {
    let time = text(item, "paidOn")?;
    in_range(time, from, to)?;
    let Some(quantity) = item.get("quantity").and_then(decimal) else { return Some(skipped("Invalid dividend", time)); };
    let Some(per_share) = item.get("grossAmountPerShare").and_then(decimal) else { return Some(skipped("Invalid dividend", time)); };
    if quantity <= Decimal::ZERO || per_share < Decimal::ZERO {
        return Some(skipped("Invalid dividend", time));
    }
    let Some(isin) = item.get("instrument").and_then(|instrument| text(instrument, "isin")).filter(|isin| !isin.is_empty()) else {
        return Some(skipped("Missing ISIN", time));
    };
    let Some(currency_code) = text(item, "tickerCurrency").or_else(|| item.get("instrument").and_then(|instrument| text(instrument, "currency"))).filter(|value| currency(value)) else {
        return Some(skipped("Invalid dividend currency", time));
    };
    let kind = text(item, "type").unwrap_or("ORDINARY");
    let reference = text(item, "reference").unwrap_or("");
    let shares = plain(quantity);
    let price = plain(per_share);
    let total = plain(quantity * per_share);
    // The history list does not return withholding. A blank column is zero in
    // the importer; it is not a separately reported tax.
    Some(fields(&[&format!("Dividend ({kind})"), time, isin, &shares, &price, currency_code, &total, currency_code, reference, "0", "1", "", ""]))
}

fn transaction_row(item: &Value, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<String> {
    let time = text(item, "dateTime")?;
    in_range(time, from, to)?;
    let kind = text(item, "type").unwrap_or("UNKNOWN");
    let action = match kind {
        "INTEREST_ON_FREE_CASH" => "Interest on cash",
        "LENDING_INTEREST" => "Lending interest",
        "DEPOSIT" => "Deposit",
        "WITHDRAW" => "Withdrawal",
        other => other,
    };
    let amount = item.get("amount").and_then(decimal).unwrap_or(Decimal::ZERO);
    let currency_code = text(item, "currency").unwrap_or("");
    if matches!(action, "Interest on cash" | "Lending interest") && (!currency(currency_code) || amount <= Decimal::ZERO) {
        return Some(skipped(action, time));
    }
    let total = plain(amount.abs());
    let reference = text(item, "reference").unwrap_or("");
    Some(fields(&[action, time, "", "0", "0", "", &total, currency_code, reference, "0", "1", "", ""]))
}

fn tax_sum(taxes: Option<&Vec<Value>>, name: &str, matching: bool) -> Option<Option<(Decimal, String)>> {
    let Some(taxes) = taxes else { return Some(None); };
    let mut total = Decimal::ZERO;
    let mut seen = None;
    for tax in taxes {
        let tax_name = text(tax, "name").unwrap_or("");
        if (tax_name == name) != matching { continue; }
        let Some(amount) = tax.get("quantity").and_then(decimal) else { return None; };
        let Some(tax_currency) = text(tax, "currency") else { return None; };
        if seen.get_or_insert(tax_currency) != &tax_currency { return None; }
        total += amount;
    }
    Some(seen.map(|currency| (total, currency.to_owned())))
}

fn catalog_field(instruments: &Value, ticker: &str, key: &str) -> Option<String> {
    instruments.as_array()?.iter().find(|item| text(item, "ticker") == Some(ticker))
        .and_then(|item| text(item, key).map(str::to_owned))
}

fn in_range(time: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(time).ok()?.with_timezone(&Utc);
    (parsed >= from && parsed < to).then_some(parsed)
}

fn skipped(action: &str, time: &str) -> String {
    fields(&[action, time, "", "", "", "", "", "", "", "", "", "", ""])
}

fn fields(values: &[&str]) -> String {
    values.iter().map(|value| {
        if value.contains([',', '"', '\n', '\r']) { format!("\"{}\"", value.replace('"', "\"\"")) } else { (*value).to_owned() }
    }).collect::<Vec<_>>().join(",")
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|text| !text.is_empty())
}

fn id_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn plain(value: Decimal) -> String { value.normalize().to_string() }

fn currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// Broker `nextPagePath` values stay on the same history feed. Anything else is
/// not followed.
pub(super) fn next_history_path(feed: &str, next: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(next) = next.map(str::trim).filter(|next| !next.is_empty() && *next != "null") else { return Ok(None); };
    let path = if let Some(rest) = next.strip_prefix("/api/v0") {
        rest.to_owned()
    } else if let Some(query) = next.strip_prefix('?') {
        format!("/equity/history/{feed}?{query}")
    } else if next.starts_with("/equity/history/") {
        next.to_owned()
    } else {
        anyhow::bail!("Trading 212 returned a history page link outside the history API");
    };
    let base = path.split_once('?').map_or(path.as_str(), |(base, _)| base);
    if base != format!("/equity/history/{feed}") || path.contains("//") {
        anyhow::bail!("Trading 212 returned a history page link outside the history API");
    }
    Ok(Some(path))
}

pub(super) fn history_feed(phase: &str) -> Option<&'static str> {
    match phase {
        "history_orders" => Some("orders"),
        "history_dividends" => Some("dividends"),
        "history_transactions" => Some("transactions"),
        _ => None,
    }
}

pub(super) fn next_history_phase(phase: &str) -> Option<&'static str> {
    match phase {
        "history_orders" => Some("history_dividends"),
        "history_dividends" => Some("history_transactions"),
        _ => None,
    }
}

pub(super) const HISTORY_COMPLETE: &str = "history-complete";
