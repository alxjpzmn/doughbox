use std::collections::HashMap;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::{
    database::{
        db_client,
        models::trade::{Trade, TradeWithHash},
        queries::{listing_change::get_listing_changes, QueryFilter},
    },
    services::instruments::identifiers::get_changed_identifier,
};


pub async fn find_similar_trade(user_id: &crate::database::models::auth::UserId, trade: &Trade) -> anyhow::Result<Option<TradeWithHash>> { let client = db_client().await?;

let query = r#"
    SELECT broker, date, isin, avg_price_per_unit, eur_avg_price_per_unit, units, 
           direction, security_type, currency, date_added, fees, 
           withholding_tax, withholding_tax_currency, hash
    FROM trade WHERE user_id = $1 AND isin = $2 AND date = $3 AND units = $4 AND avg_price_per_unit = $5
"#;

let row = client
    .query_opt(
        query,
        &[
                &user_id.as_str(),
            &trade.isin,
            &trade.date,
            &trade.units,
            &trade.avg_price_per_unit,
        ],
    )
    .await
    .ok()
    .unwrap();

if let Some(row) = row {
    let found_trade = TradeWithHash {
        broker: row.get("broker"),
        date: row.get("date"),
        isin: row.get("isin"),
        avg_price_per_unit: row.get("avg_price_per_unit"),
        eur_avg_price_per_unit: row.get("eur_avg_price_per_unit"),
        units: row.get("units"),
        direction: row.get("direction"),
        security_type: row.get("security_type"),
        currency: row.get("currency"),
        date_added: row.get("date_added"),
        fees: row.get("fees"),
        withholding_tax: row.get("withholding_tax"),
        withholding_tax_currency: row.get("withholding_tax_currency"),
        hash: row.get("hash"),
    };

    return Ok(Some(found_trade));
}
Ok(None) }

pub async fn get_trades(user_id: &crate::database::models::auth::UserId, filter: QueryFilter) -> anyhow::Result<Vec<Trade>> { let client = db_client().await?;
let listing_changes = get_listing_changes().await?;

let mut statement = String::from(
    "SELECT broker, date, units, avg_price_per_unit, eur_avg_price_per_unit, \
     security_type, direction, currency, isin, date_added, fees, \
     withholding_tax, withholding_tax_currency \
     FROM trade WHERE user_id = $1",
);
let owner = user_id.as_str();
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&owner];
super::append_common_filters(&mut statement, &mut params, &filter);
statement.push_str(" ORDER BY date DESC");
if let Some(limit) = filter.limit {
    statement.push_str(&format!(" LIMIT {}", limit));
}

let rows = client.query(&statement, &params).await?;
let mut trades: Vec<Trade> = vec![];

for row in rows {
    let isin = get_changed_identifier(&row.get::<_, String>("isin"), listing_changes.clone());
    if let Some(ref wanted) = filter.isin {
        if &isin != wanted {
            continue;
        }
    }
    trades.push(trade_from_row(&row, isin));
}

Ok(trades) }

/// Reads a bounded ledger page, filtering canonical identifiers before pagination.
/// Unlike `get_trades`, the upper date bound is exclusive and `filter.limit` is ignored.
pub(crate) async fn get_trades_page(
    user_id: &crate::database::models::auth::UserId,
    filter: &QueryFilter,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<Trade>> {
    let client = db_client().await?;
    let listing_changes = get_listing_changes().await?;
    // Preserve get_changed_identifier's first-match, one-step mapping.
    let mut canonical_identifiers = HashMap::with_capacity(listing_changes.len());
    for change in &listing_changes {
        canonical_identifiers
            .entry(change.from_identifier.as_str())
            .or_insert(change.to_identifier.as_str());
    }
    let mut matching_identifiers = Vec::new();
    if let Some(wanted) = filter.isin.as_deref() {
        for (&source, &target) in &canonical_identifiers {
            if target == wanted {
                matching_identifiers.push(source);
            }
        }
        if !canonical_identifiers.contains_key(wanted) {
            matching_identifiers.push(wanted);
        }
    }

    let mut statement = String::from(
        "SELECT broker, date, units, avg_price_per_unit, eur_avg_price_per_unit, \
         security_type, direction, currency, isin, date_added, fees, \
         withholding_tax, withholding_tax_currency \
         FROM trade WHERE user_id = $1",
    );
    let owner = user_id.as_str();
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&owner];
    if let Some(broker) = &filter.broker {
        params.push(broker);
        statement.push_str(&format!(" AND broker = ${}", params.len()));
    }
    if let Some(direction) = &filter.direction {
        params.push(direction);
        statement.push_str(&format!(" AND direction = ${}", params.len()));
    }
    if let Some(from_date) = &filter.from_date {
        params.push(from_date);
        statement.push_str(&format!(" AND date >= ${}", params.len()));
    }
    if let Some(until_date) = &filter.until_date {
        params.push(until_date);
        statement.push_str(&format!(" AND date < ${}", params.len()));
    }
    if filter.isin.is_some() {
        params.push(&matching_identifiers);
        statement.push_str(&format!(" AND isin = ANY(${})", params.len()));
    }
    statement.push_str(" ORDER BY date DESC, hash DESC");
    params.push(&limit);
    statement.push_str(&format!(" LIMIT ${}", params.len()));
    params.push(&offset);
    statement.push_str(&format!(" OFFSET ${}", params.len()));

    let rows = client.query(&statement, &params).await?;
    Ok(rows
        .iter()
        .map(|row| {
            let source = row.get::<_, String>("isin");
            let isin = match canonical_identifiers.get(source.as_str()) {
                Some(target) => (*target).to_owned(),
                None => source,
            };
            trade_from_row(row, isin)
        })
        .collect())
}

fn trade_from_row(row: &tokio_postgres::Row, isin: String) -> Trade {
    let withholding_tax = row.get::<_, Decimal>("withholding_tax");
    let withholding_tax_currency = if withholding_tax == dec!(0) {
        "EUR".to_string()
    } else {
        row.get::<_, String>("withholding_tax_currency")
    };
    Trade {
        broker: row.get("broker"),
        date: row.get("date"),
        units: row.get("units"),
        avg_price_per_unit: row.get("avg_price_per_unit"),
        eur_avg_price_per_unit: row.get("eur_avg_price_per_unit"),
        security_type: row.get("security_type"),
        direction: row.get("direction"),
        currency: row.get("currency"),
        isin,
        date_added: row.get("date_added"),
        fees: row.get("fees"),
        withholding_tax,
        withholding_tax_currency,
    }
}

