use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::database::{db_client, models::performance::PerformanceSignal};

pub async fn add_performance_signal_to_db(
    performance_signal: PerformanceSignal,
) -> anyhow::Result<()> {
    let client = db_client().await?;
    let components = performance_signal.components.as_ref().map(serde_json::to_string).transpose()?;

    client
        .execute(
            "INSERT INTO performance (date, total_value, total_invested, gross_invested, components) values ($1, $2, $3, $4, $5::text::jsonb)",
            &[
                &performance_signal.date,
                &performance_signal.total_value,
                &performance_signal.total_invested,
                &performance_signal.gross_invested,
                &components,
            ],
        )
        .await?;

    Ok(())
}

pub async fn get_performance_signals() -> anyhow::Result<Vec<PerformanceSignal>> {
    let client = db_client().await?;

    let statement: String =
        "SELECT DISTINCT ON (date_trunc('day', date)) date, total_value, total_invested, gross_invested, components::text
    FROM performance
    ORDER BY date_trunc('day', date), date DESC;"
            .to_string();

    let rows = client.query(&statement, &[]).await?;

    rows.into_iter().map(signal_from_row).collect()
}

pub async fn get_latest_performance_signal() -> anyhow::Result<Option<PerformanceSignal>> {
    let client = db_client().await?;

    let statement = "SELECT date, total_value, total_invested, gross_invested, components::text
                     FROM performance
                     ORDER BY date DESC
                     LIMIT 1;";

    Ok(client
        .query_opt(statement, &[])
        .await?
        .map(signal_from_row)
        .transpose()?)
}

fn signal_from_row(row: tokio_postgres::Row) -> anyhow::Result<PerformanceSignal> {
    let components: Option<String> = row.get(4);
    Ok(PerformanceSignal {
        date: row.get::<_, DateTime<Utc>>(0),
        total_value: row.get::<_, Decimal>(1),
        total_invested: row.get::<_, Decimal>(2),
        gross_invested: row.get::<_, Option<Decimal>>(3),
        components: components.map(|json| serde_json::from_str(&json)).transpose()?,
    })
}
