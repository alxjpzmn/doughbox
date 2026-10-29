use crate::{
    database::{
        db_client,
        models::{
            asset::{InterestOrigin, TaxTreatment},
            interest::{InterestPayment, InterestRecord},
        },
        queries::QueryFilter,
    },
    services::shared::util::hash_string,
};

/// Check if an interest payment with the given hash already exists
pub async fn interest_exists_by_hash(hash: &str) -> anyhow::Result<bool> {
    let client = db_client().await?;
    let row = client
        .query_opt("SELECT 1 FROM interest WHERE id = $1", &[&hash])
        .await?;
    Ok(row.is_some())
}

/// Add interest payment to database, returns true if inserted, false if duplicate
pub async fn add_interest_to_db(
    interest_payment: InterestPayment,
    transaction_id: Option<&str>,
) -> anyhow::Result<bool> {
    let client = db_client().await?;

    // Generate hash - include transaction_id if available
    let hash = if let Some(tx_id) = transaction_id {
        hash_string(
            format!(
                "{}{}{}{}{}",
                interest_payment.date,
                interest_payment.amount,
                interest_payment.broker,
                interest_payment.principal,
                tx_id
            )
            .as_str(),
        )
    } else {
        hash_string(
            format!(
                "{}{}{}{}",
                interest_payment.date,
                interest_payment.amount,
                interest_payment.broker,
                interest_payment.principal
            )
            .as_str(),
        )
    };

    // Check if already exists
    if interest_exists_by_hash(&hash).await? {
        return Ok(false);
    }

    let result = client.execute(
            "INSERT INTO interest (id, date, amount, broker, principal, currency, amount_eur, withholding_tax, withholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &interest_payment.date, &interest_payment.amount, &interest_payment.broker, &interest_payment.principal, &interest_payment.currency, &interest_payment.amount_eur, &interest_payment.withholding_tax, &interest_payment.withholding_tax_currency],
        )
    .await?;

    // Return true if a row was actually inserted
    Ok(result == 1)
}

pub async fn get_interest(filter: QueryFilter) -> anyhow::Result<Vec<InterestRecord>> {
    let client = db_client().await?;

    let mut statement = String::from(
        "SELECT id, asset_id, date, amount, broker, principal, currency, amount_eur, \
         withholding_tax, withholding_tax_currency, tax_treatment, origin \
         FROM interest WHERE 1=1",
    );
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
    super::append_common_filters(&mut statement, &mut params, &filter);
    statement.push_str(" ORDER BY date DESC");

    let rows = client.query(&statement, &params).await?;
    Ok(rows
        .iter()
        .map(|row| InterestRecord {
            id: row.get("id"),
            asset_id: row.get("asset_id"),
            date: row.get("date"),
            amount: row.get("amount"),
            broker: row.get("broker"),
            principal: row.get("principal"),
            currency: row.get("currency"),
            amount_eur: row.get("amount_eur"),
            withholding_tax: row.get("withholding_tax"),
            withholding_tax_currency: row.get("withholding_tax_currency"),
            tax_treatment: match row.get::<_, &str>("tax_treatment") {
                "Included" => TaxTreatment::Included,
                _ => TaxTreatment::Excluded,
            },
            origin: match row.get::<_, &str>("origin") {
                "Manual" => InterestOrigin::Manual,
                _ => InterestOrigin::Imported,
            },
        })
        .collect())
}
