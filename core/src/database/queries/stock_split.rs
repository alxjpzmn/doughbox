use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::{database::db_client, services::instruments::stock_splits::StockSplit};

pub async fn add_stock_split_to_db(stock_split: StockSplit) -> anyhow::Result<()> {
    let client = db_client().await?;

    client.execute(
            "INSERT INTO stock_splits (id, ex_date, from_factor, to_factor, isin, date_added) values ($1, $2, $3, $4, $5, $6) ON CONFLICT(id) DO NOTHING",
            &[&stock_split.id, &stock_split.ex_date, &stock_split.from_factor, &stock_split.to_factor, &stock_split.isin, &Utc::now()],
        )
    .await?;

    Ok(())
}

pub async fn get_stock_splits() -> anyhow::Result<Vec<StockSplit>> {
    let client = db_client().await?;

    let rows = client.query(r#"select * from stock_splits"#, &[]).await?;

    let mut stock_splits: Vec<StockSplit> = vec![];

    for row in rows {
        let stock_split = StockSplit {
            id: row.get::<usize, String>(0),
            ex_date: row.get::<usize, DateTime<Utc>>(1),
            from_factor: row.get::<usize, Decimal>(2),
            to_factor: row.get::<usize, Decimal>(3),
            isin: row.get::<usize, String>(4),
        };

        stock_splits.push(stock_split);
    }

    Ok(stock_splits)
}
