use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::database::{db_client, models::performance::PerformanceSignal};

pub async fn add_performance_signal_to_db(
    performance_signal: PerformanceSignal,
) -> anyhow::Result<()> {
    let client = db_client().await?;

    client
        .execute(
            "INSERT INTO performance (date, total_value, total_invested) values ($1, $2, $3)",
            &[
                &performance_signal.date,
                &performance_signal.total_value,
                &performance_signal.total_invested,
            ],
        )
        .await?;

    Ok(())
}

pub async fn get_performance_signals() -> anyhow::Result<Vec<PerformanceSignal>> {
    let client = db_client().await?;

    let statement: String =
        "SELECT DISTINCT ON (date_trunc('day', date)) date, total_value, total_invested
    FROM performance
    ORDER BY date_trunc('day', date), date DESC;"
            .to_string();

    let rows = client.query(&statement, &[]).await?;

    let mut performance_signals: Vec<PerformanceSignal> = vec![];

    for row in rows {
        let trade = PerformanceSignal {
            date: row.get::<usize, DateTime<Utc>>(0),
            total_value: row.get::<usize, Decimal>(1),
            total_invested: row.get::<usize, Decimal>(2),
        };
        performance_signals.push(trade);
    }

    Ok(performance_signals)
}

pub async fn get_latest_performance_signal() -> anyhow::Result<Option<PerformanceSignal>> {
    let client = db_client().await?;

    let statement = "SELECT date, total_value, total_invested
                     FROM performance
                     ORDER BY date DESC
                     LIMIT 1;";

    Ok(client
        .query_opt(statement, &[])
        .await?
        .map(|row| PerformanceSignal {
            date: row.get::<usize, DateTime<Utc>>(0),
            total_value: row.get::<usize, Decimal>(1),
            total_invested: row.get::<usize, Decimal>(2),
        }))
}
