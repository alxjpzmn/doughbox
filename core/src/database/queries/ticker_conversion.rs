use crate::database::db_client;

pub async fn get_isin_from_symbol(symbol: &str) -> anyhow::Result<String> {
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
