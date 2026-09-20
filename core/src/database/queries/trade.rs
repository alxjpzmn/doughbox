use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::{
    database::{
        db_client,
        models::trade::{MonthlyNetInflow, Trade, TradeWithHash},
        queries::{listing_change::get_listing_changes, QueryFilter},
    },
    services::instruments::identifiers::get_changed_identifier,
};

pub async fn get_realized_return() -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    let result = client
        .query_one(
            "select SUM(t.eur_avg_price_per_unit * t.units) from trade t where direction = 'Sell'",
            &[],
        )
        .await?;

    Ok(result.try_get::<usize, Decimal>(0).unwrap_or(dec!(0.0)))
}

pub async fn find_similar_trade(trade: &Trade) -> anyhow::Result<Option<TradeWithHash>> {
    let client = db_client().await?;

    let query = r#"
        SELECT broker, date, isin, avg_price_per_unit, eur_avg_price_per_unit, units, 
               direction, security_type, currency, date_added, fees, 
               withholding_tax, withholding_tax_currency, hash
        FROM trade
        WHERE isin = $1 AND date = $2 AND units = $3 AND avg_price_per_unit = $4
    "#;

    let row = client
        .query_opt(
            query,
            &[
                &trade.isin,
                &trade.date,
                &trade.units,
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
            units: row.get("units"),
            direction: row.get("direction"),
            security_type: row.get("security_type"),
            currency: row.get("currency"),
            date_added: row.get("date_added"),
            fees: row.get("fees"),
            withholding_tax: row.get("withholding_tax"),
            withholding_tax_currency: row.get("withholding_tax_currency"),
            hash: row.get("hash"),
        };

        return Ok(Some(found_trade));
    }
    Ok(None)
}

pub async fn get_trades(filter: QueryFilter) -> anyhow::Result<Vec<Trade>> {
    let client = db_client().await?;
    let listing_changes = get_listing_changes().await?;

    let mut statement = String::from(
        "SELECT broker, date, units, avg_price_per_unit, eur_avg_price_per_unit, \
         security_type, direction, currency, isin, date_added, fees, \
         withholding_tax, withholding_tax_currency \
         FROM trade WHERE 1=1",
    );
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
    super::append_common_filters(&mut statement, &mut params, &filter);
    statement.push_str(" ORDER BY date DESC");
    if let Some(limit) = filter.limit {
        statement.push_str(&format!(" LIMIT {}", limit));
    }

    let rows = client.query(&statement, &params).await?;
    let mut trades: Vec<Trade> = vec![];

    for row in rows {
        let withholding_tax = row.get::<_, Decimal>("withholding_tax");
        let withholding_tax_currency = if withholding_tax == dec!(0) {
            "EUR".to_string()
        } else {
            row.get::<_, String>("withholding_tax_currency")
        };
        let isin = get_changed_identifier(&row.get::<_, String>("isin"), listing_changes.clone());
        if let Some(ref wanted) = filter.isin {
            if &isin != wanted {
                continue;
            }
        }
        trades.push(Trade {
            broker: row.get("broker"),
            date: row.get("date"),
            units: row.get("units"),
            avg_price_per_unit: row.get("avg_price_per_unit"),
            eur_avg_price_per_unit: row.get("eur_avg_price_per_unit"),
            security_type: row.get("security_type"),
            direction: row.get("direction"),
            currency: row.get("currency"),
            isin,
            date_added: row.get("date_added"),
            fees: row.get("fees"),
            withholding_tax,
            withholding_tax_currency,
        });
    }

    Ok(trades)
}

pub async fn get_total_invested_value() -> anyhow::Result<Decimal> {
    let client = db_client().await?;

    let result = client
        .query_one(
            "select SUM(t.eur_avg_price_per_unit * t.units) from trade t where direction = 'Buy'",
            &[],
        )
        .await?;

    Ok(result.try_get::<usize, Decimal>(0).unwrap_or(dec!(0.0)))
}

pub async fn get_monthly_net_inflow() -> anyhow::Result<Vec<MonthlyNetInflow>> {
    let client = db_client().await?;
    let rows = client
        .query(
            "SELECT to_char(date_trunc('month', date AT TIME ZONE 'UTC'), 'YYYY-MM') AS month,
                    COALESCE(ROUND(SUM(
                        CASE
                            WHEN direction = 'Buy' THEN eur_avg_price_per_unit * units
                            WHEN direction = 'Sell' THEN -(eur_avg_price_per_unit * units)
                            ELSE 0
                        END
                    ), 2), 0) AS net_eur
             FROM trade
             GROUP BY 1
             ORDER BY 1",
            &[],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|row| MonthlyNetInflow {
            month: row.get("month"),
            net_eur: row.get("net_eur"),
        })
        .collect())
}
