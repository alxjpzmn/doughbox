use crate::database::db_client;

pub async fn query_isin_from_symbol(symbol: &str) -> anyhow::Result<String> {
    println!("Getting ISIN for symbol {}...", &symbol);

    let client = db_client().await?;

    let statement = format!(
        "SELECT isin from ticker_conversion where ticker = '{}'",
        symbol
    );

    let result = client.query_one(&statement, &[]).await?;

    Ok(result
        .try_get::<usize, String>(0)
        .unwrap_or("Unidentified".to_string()))
}

pub async fn query_symbol_from_isin(isin: &str) -> anyhow::Result<String> {
    let client = db_client().await?;

    let statement = "SELECT ticker FROM ticker_conversion WHERE isin = $1";

    let result = client.query_one(statement, &[&isin]).await;

    // Extract the ticker from the row, or return "Unidentified" if not found
    let ticker = match result {
        Ok(row) => {
            // Try to get the ticker value from the first column (index 0)
            row.try_get::<_, String>(0)
                .map_err(|e| anyhow::anyhow!("Failed to parse ticker from database: {}", e))?
        }
        Err(_e) => "Unidentified".to_string(),
    };

    Ok(ticker)
}

pub async fn insert_ticker_conversion(isin: &str, symbol: &str) -> anyhow::Result<()> {
    let client = db_client().await?;

    client.execute(
        "INSERT INTO ticker_conversion (isin, ticker) values ($1, $2) ON CONFLICT(id) DO NOTHING",
        &[&isin, &symbol])
    .await?;

    Ok(())
}
