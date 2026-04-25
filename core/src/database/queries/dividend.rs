use crate::{
    database::{db_client, models::dividend::Dividend},
    services::shared::util::hash_string,
};

/// Check if a dividend with the given hash already exists in the database
pub async fn dividend_exists_by_hash(hash: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    let row = client
        .query_opt("SELECT 1 FROM dividend WHERE id = $1", &[&hash])
        .await?;
    Ok(row.is_some())
}

/// Add dividend to database, returns true if inserted, false if duplicate
pub async fn add_dividend_to_db(dividend: Dividend, transaction_id: Option<&str>) -> anyhow::Result<bool> {
    let client = db_client().await?;

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

    // Check if already exists
    if dividend_exists_by_hash(&hash).await? {
        return Ok(false);
    }

    let result = client.execute(
            "INSERT INTO dividend (id, isin, date, amount, broker, currency, amount_eur, withholding_tax, withholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &dividend.isin, &dividend.date, &dividend.amount, &dividend.broker, &dividend.currency, &dividend.amount_eur, &dividend.withholding_tax, &dividend.withholding_tax_currency],
        )
    .await?;
    
    // Return true if a row was actually inserted
    Ok(result == 1)
}
