use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::database::{
    db_client,
    models::trade::{Trade, TradeWithHash},
};

pub async fn get_total_sell_value() -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    let result = client
        .query_one(
            "select SUM(t.eur_avg_price_per_unit * t.no_units) from trade t where direction = 'Sell'",
            &[],
        )
        .await?;

    Ok(result.try_get::<usize, Decimal>(0).unwrap_or(dec!(0.0)))
}

pub async fn find_similar_trade(trade: &Trade) -> anyhow::Result<Option<TradeWithHash>> {
    let client = db_client().await?;

    let query = r#"
        SELECT broker, date, isin, avg_price_per_unit, eur_avg_price_per_unit, no_units, 
               direction, security_type, currency_denomination, date_added, fees, 
               withholding_tax, witholding_tax_currency, hash
        FROM trade
        WHERE isin = $1 AND date = $2 AND no_units = $3 AND avg_price_per_unit = $4
    "#;

    let row = client
        .query_opt(
            query,
            &[
                &trade.isin,
                &trade.date,
                &trade.no_units,
                &trade.avg_price_per_unit,
            ],
        )
        .await
        .ok()
        .unwrap();

    if let Some(row) = row {
        let found_trade = TradeWithHash {
            broker: row.get("broker"),
            date: row.get("date"),
            isin: row.get("isin"),
            avg_price_per_unit: row.get("avg_price_per_unit"),
            eur_avg_price_per_unit: row.get("eur_avg_price_per_unit"),
            no_units: row.get("no_units"),
            direction: row.get("direction"),
            security_type: row.get("security_type"),
            currency_denomination: row.get("currency_denomination"),
            date_added: row.get("date_added"),
            fees: row.get("fees"),
            withholding_tax: row.get("withholding_tax"),
            witholding_tax_currency: row.get("witholding_tax_currency"),
            hash: row.get("hash"),
        };

        return Ok(Some(found_trade));
    }
    Ok(None)
}

pub async fn get_total_invested_value() -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    let result = client
        .query_one(
            "select SUM(t.eur_avg_price_per_unit * t.no_units) from trade t where direction = 'Buy'",
            &[],
        )
        .await?;

    Ok(result.try_get::<usize, Decimal>(0).unwrap_or(dec!(0.0)))
}
