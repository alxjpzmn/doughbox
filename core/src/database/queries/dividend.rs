use tokio_postgres::GenericClient;

use crate::{
    database::{
        db_client,
        models::dividend::Dividend,
        queries::{listing_change::get_listing_changes, QueryFilter},
    },
    services::{instruments::identifiers::get_changed_identifier, shared::util::hash_string},
};

/// Add dividend to database, returns true if inserted, false if duplicate
pub async fn add_dividend_to_db(user_id: &crate::database::models::auth::UserId, dividend: Dividend,
transaction_id: Option<&str>,) -> anyhow::Result<bool> { let client = db_client().await?;

// Generate hash - include transaction_id if available for better deduplication
let hash = if let Some(tx_id) = transaction_id {
    hash_string(
        format!(
            "{}{}{}{}{}",
            dividend.isin, dividend.date, dividend.amount, dividend.broker, tx_id
        )
        .as_str(),
    )
} else {
    hash_string(
        format!(
            "{}{}{}{}",
            dividend.isin, dividend.date, dividend.amount, dividend.broker
        )
        .as_str(),
    )
};

insert_with_id(user_id, &client, &hash, &dividend).await }

pub async fn insert_with_id(
    user_id: &crate::database::models::auth::UserId,
    db: &(impl GenericClient + Sync),
    id: &str,
    dividend: &Dividend,
) -> anyhow::Result<bool> {
    let inserted = db.execute(
        "INSERT INTO dividend (user_id, id, isin, date, amount, broker, currency, amount_eur, withholding_tax, withholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT(user_id,id) DO NOTHING",
        &[&user_id.as_str(), &id, &dividend.isin, &dividend.date, &dividend.amount, &dividend.broker, &dividend.currency, &dividend.amount_eur, &dividend.withholding_tax, &dividend.withholding_tax_currency],
    ).await?;
    Ok(inserted == 1)
}

pub async fn get_dividends(user_id: &crate::database::models::auth::UserId, filter: QueryFilter) -> anyhow::Result<Vec<Dividend>> { let client = db_client().await?;
let listing_changes = get_listing_changes().await?;

let mut statement = String::from(
    "SELECT isin, date, amount, broker, currency, amount_eur, \
     withholding_tax, withholding_tax_currency FROM dividend WHERE user_id = $1",
);
let owner = user_id.as_str();
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&owner];
super::append_common_filters(&mut statement, &mut params, &filter);
statement.push_str(" ORDER BY date DESC");

let rows = client.query(&statement, &params).await?;
let mut dividends = Vec::new();
for row in rows {
    let isin = get_changed_identifier(&row.get::<_, String>("isin"), listing_changes.clone());
    if let Some(ref wanted) = filter.isin {
        if &isin != wanted {
            continue;
        }
    }
    dividends.push(Dividend {
        isin,
        date: row.get("date"),
        amount: row.get("amount"),
        broker: row.get("broker"),
        currency: row.get("currency"),
        amount_eur: row.get("amount_eur"),
        withholding_tax: row.get("withholding_tax"),
        withholding_tax_currency: row.get("withholding_tax_currency"),
    });
}
Ok(dividends) }
