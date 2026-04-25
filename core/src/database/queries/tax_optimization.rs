use chrono::{DateTime, Utc};

use crate::{
    database::{db_client, models::tax_optimization::TaxOptimization},
    services::shared::util::hash_string,
};

/// Check if a tax optimization with the given hash already exists
pub async fn tax_optimization_exists_by_hash(hash: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    let row = client
        .query_opt("SELECT 1 FROM tax_optimizations WHERE id = $1", &[&hash])
        .await?;
    Ok(row.is_some())
}

/// Add tax optimization to database, returns true if inserted, false if duplicate
pub async fn add_tax_optimization_to_db(tax_optimization: TaxOptimization) -> anyhow::Result<bool> {
    let client = db_client().await?;

    let hash = hash_string(
        format!(
            "{}{}{}{}{}",
            tax_optimization.broker,
            tax_optimization.date,
            tax_optimization.amount,
            tax_optimization.tax_type,
            tax_optimization.transaction_id.as_deref().unwrap_or("")
        )
        .as_str(),
    );

    // Check if already exists
    if tax_optimization_exists_by_hash(&hash).await? {
        return Ok(false);
    }

    let result = client
        .execute(
            "INSERT INTO tax_optimizations (id, date, broker, amount, currency, amount_eur, tax_type, description, transaction_id) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[
                &hash,
                &tax_optimization.date,
                &tax_optimization.broker,
                &tax_optimization.amount,
                &tax_optimization.currency,
                &tax_optimization.amount_eur,
                &tax_optimization.tax_type,
                &tax_optimization.description,
                &tax_optimization.transaction_id,
            ],
        )
        .await?;

    // Return true if a row was actually inserted
    Ok(result == 1)
}

pub async fn get_tax_optimizations_by_date_range(
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
) -> anyhow::Result<Vec<TaxOptimization>> {
    let client = db_client().await?;

    let rows = client
        .query(
            "SELECT date, broker, amount, currency, amount_eur, tax_type, description, transaction_id FROM tax_optimizations WHERE date >= $1 AND date <= $2",
            &[&start_date, &end_date],
        )
        .await?;

    let mut tax_optimizations = Vec::new();
    for row in rows {
        tax_optimizations.push(TaxOptimization {
            date: row.get(0),
            broker: row.get(1),
            amount: row.get(2),
            currency: row.get(3),
            amount_eur: row.get(4),
            tax_type: row.get(5),
            description: row.get(6),
            transaction_id: row.get(7),
        });
    }

    Ok(tax_optimizations)
}
