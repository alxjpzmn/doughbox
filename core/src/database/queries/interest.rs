use log::info;

use crate::{
    database::{db_client, models::interest::InterestPayment},
    services::shared::util::hash_string,
};

pub async fn add_interest_to_db(interest_payment: InterestPayment) -> anyhow::Result<()> {
    let client = db_client().await?;

    let hash = hash_string(
        format!(
            "{}{}{}{}",
            interest_payment.date,
            interest_payment.amount,
            interest_payment.broker,
            interest_payment.principal
        )
        .as_str(),
    );

    // generate id based on date, isin, broker and amount
    client.execute(
            "INSERT INTO interest (id, date, amount, broker, principal, currency, amount_eur, withholding_tax, witholding_tax_currency) values ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT(id) DO NOTHING",
            &[&hash, &interest_payment.date, &interest_payment.amount, &interest_payment.broker, &interest_payment.principal, &interest_payment.currency, &interest_payment.amount_eur, &interest_payment.withholding_tax, &interest_payment.witholding_tax_currency],
        )
    .await?;

    info!(target: "import", "💵 Interest payment added: {:?}", interest_payment);
    Ok(())
}
