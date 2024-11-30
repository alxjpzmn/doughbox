use chrono::Utc;
use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use serde_json::json;

use crate::util::{
    constants::OUT_DIR,
    db_helpers::{get_all_trades, get_stock_splits, Trade},
    general_helpers::get_env_variable,
    market_data_helpers::{
        fetch_fred_data_set, get_current_equity_price, get_current_security_name,
        get_fred_value_for_date,
    },
    performance_helpers::{
        get_title_performance, round_to_decimals, simulate_alternate_purchase, TradeGroup,
        TradeGroupPerformance,
    },
};

#[derive(Debug, Serialize)]
pub struct PlOverview {
    pub generated_at: i64,
    pub total_actual: Decimal,
    pub total_simulated_pl: Decimal,
    pub total_alpha: Decimal,
    pub position_pl: Vec<MergedPositionPl>,
}

#[derive(Debug, Clone)]
pub struct PositionPl {
    pub isin: String,
    pub name: String,
    pub broker: String,
    pub unrealized_pl: Decimal,
    pub realized_pl: Decimal,
    pub pl: Decimal,
    pub return_on_equity: Decimal,
    pub invested_amount: Decimal,
}

#[derive(Debug, Serialize, Clone)]
pub struct MergedPositionPl {
    pub isin: String,
    pub name: String,
    pub unrealized_pl: Decimal,
    pub realized_pl: Decimal,
    pub pl: Decimal,
    pub pl_simulated: Decimal,
    pub real_vs_sim: Decimal,
    pub return_on_equity: Decimal,
    pub invested_amount: Decimal,
}

pub async fn get_performance() -> anyhow::Result<PlOverview> {
    let fred_token_set = get_env_variable("FRED_TOKEN").is_some();

    let mut trades: Vec<Trade> = get_all_trades(None).await?;
    trades.sort_unstable_by_key(|item| (item.isin.clone(), item.broker.clone()));
    let grouped_trades: Vec<TradeGroup> = trades
        .iter()
        .chunk_by(|x| (x.broker.clone(), x.isin.clone()))
        .into_iter()
        .map(|((broker, isin), group)| TradeGroup {
            broker,
            isin,
            trades: group
                .map(|u| Trade {
                    isin: u.isin.clone(),
                    broker: u.broker.clone(),
                    date: u.date,
                    no_units: u.no_units,
                    avg_price_per_unit: u.avg_price_per_unit,
                    eur_avg_price_per_unit: u.eur_avg_price_per_unit,
                    security_type: u.security_type.clone(),
                    direction: u.direction.clone(),
                    currency_denomination: u.currency_denomination.clone(),
                    date_added: Utc::now(),
                    fees: dec!(0.0),
                    withholding_tax: dec!(0.0),
                    witholding_tax_currency: "EUR".to_string(),
                })
                .collect(),
        })
        .collect();

    let mut title_performances = vec![];
    let mut simulated_sp500_title_performances: Vec<TradeGroupPerformance> = vec![];

    let mut stock_split_information = get_stock_splits().await?;

    // Fetch FRED index values if the FRED token is set
    let index_values = if fred_token_set {
        Some(fetch_fred_data_set("SP500").await?)
    } else {
        None
    };

    for grouped_trade in grouped_trades {
        let title_performance =
            get_title_performance(&grouped_trade, Utc::now(), &mut stock_split_information);
        title_performances.push(title_performance);

        let simulated_sp500_performance =
            simulate_alternate_purchase(&grouped_trade, Utc::now(), index_values.as_ref()).await?;

        if let Some(simulated_sp500_performance) = simulated_sp500_performance {
            simulated_sp500_title_performances.push(simulated_sp500_performance)
        }
    }

    let mut position_pl: Vec<PositionPl> = vec![];

    for title_performance in title_performances {
        let realized_pl = round_to_decimals(title_performance.realized_profit_eur);
        let unrealized_pl = if title_performance.units_left > dec!(0.0) {
            round_to_decimals(
                (get_current_equity_price(&title_performance.isin).await?
                    * title_performance.units_left)
                    - title_performance.average_unit_price * title_performance.units_left,
            )
        } else {
            dec!(0.0)
        };

        let pl = round_to_decimals(realized_pl + unrealized_pl);

        let performance_item = PositionPl {
            isin: title_performance.isin.clone(),
            name: get_current_security_name(&title_performance.isin.clone()).await?,
            broker: title_performance.broker,
            unrealized_pl,
            realized_pl,
            pl,
            return_on_equity: round_to_decimals(
                (pl / title_performance.invested_cash) * dec!(100.0),
            ),
            invested_amount: title_performance.invested_cash,
        };
        position_pl.push(performance_item);
    }

    position_pl.sort_by(|a, b| a.pl.partial_cmp(&b.pl).unwrap());

    let mut simulated_pl: Vec<PositionPl> = vec![];

    for simulated_performance in simulated_sp500_title_performances {
        let realized_pl = round_to_decimals(simulated_performance.realized_profit_eur);
        let unrealized_pl = if simulated_performance.units_left > dec!(0.0) {
            round_to_decimals(
                (get_fred_value_for_date(index_values.as_ref().unwrap(), Utc::now().date_naive())
                    .await?
                    * simulated_performance.units_left)
                    - simulated_performance.average_unit_price * simulated_performance.units_left,
            )
        } else {
            dec!(0.0)
        };

        let pl = round_to_decimals(realized_pl + unrealized_pl);

        let performance_item = PositionPl {
            isin: simulated_performance.isin.clone(),
            name: get_current_security_name(&simulated_performance.isin.clone()).await?,
            broker: simulated_performance.broker,
            unrealized_pl,
            realized_pl,
            pl,
            return_on_equity: round_to_decimals(
                (pl / simulated_performance.invested_cash) * dec!(100.0),
            ),
            invested_amount: simulated_performance.invested_cash,
        };
        simulated_pl.push(performance_item);
    }

    simulated_pl.sort_by(|a, b| a.pl.partial_cmp(&b.pl).unwrap());

    let total_actual_pl = &position_pl
        .clone()
        .into_iter()
        .fold(dec!(0.0), |acc, item| acc + item.pl);

    let total_simulated_pl = &simulated_pl
        .clone()
        .into_iter()
        .fold(dec!(0.0), |acc, item| acc + item.pl);

    let mut merged_positions: Vec<MergedPositionPl> = vec![];

    for item in position_pl.iter() {
        let simulated_item = match fred_token_set {
            true => simulated_pl
                .iter()
                .find(|simulated_item| {
                    simulated_item.broker == item.broker && simulated_item.isin == item.isin
                })
                .unwrap(),
            false => &PositionPl {
                isin: "".to_string(),
                name: "".to_string(),
                broker: "".to_string(),
                unrealized_pl: dec!(0),
                realized_pl: dec!(0),
                pl: dec!(0),
                return_on_equity: dec!(0),
                invested_amount: dec!(0),
            },
        };

        let merged_position_pl = MergedPositionPl {
            isin: item.isin.clone(),
            name: item.name.clone(),
            unrealized_pl: round_to_decimals(item.unrealized_pl),
            realized_pl: round_to_decimals(item.realized_pl),
            pl: round_to_decimals(item.pl),
            pl_simulated: round_to_decimals(simulated_item.pl),
            real_vs_sim: round_to_decimals(item.pl - simulated_item.pl),
            return_on_equity: round_to_decimals(item.return_on_equity),
            invested_amount: item.invested_amount,
        };
        merged_positions.push(merged_position_pl);
    }

    let pl_overview = PlOverview {
        generated_at: Utc::now().timestamp(),
        total_actual: round_to_decimals(*total_actual_pl),
        total_simulated_pl: round_to_decimals(*total_simulated_pl),
        total_alpha: round_to_decimals(total_actual_pl - total_simulated_pl),
        position_pl: merged_positions.clone(),
    };

    let pl_json = json!(&pl_overview).to_string();
    std::fs::write(format!("{}/pl.json", OUT_DIR), pl_json)?;

    let mut wtr = csv::Writer::from_path(format!("{}/pl.csv", OUT_DIR))?;
    for table_entry in &merged_positions {
        wtr.serialize(table_entry)?;
    }
    wtr.flush()?;

    Ok(pl_overview)
}
