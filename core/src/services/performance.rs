use chrono::{DateTime, Utc};

use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tabled::Tabled;

use serde::Serialize;
use serde_json::json;

use crate::database::{
    models::trade::Trade,
    queries::{
        composite::get_all_trades, instrument::get_current_instrument_price,
        stock_split::get_stock_splits,
    },
};

use super::{
    constants::OUT_DIR,
    env::get_env_variable,
    instruments::{
        names::get_current_instrument_name,
        stock_splits::{get_split_adjusted_price_per_unit, get_split_adjusted_units, StockSplit},
    },
    market_data::fred::{fetch_fred_data_set, get_fred_value_for_date, FREDResponse},
    shared::round_to_decimals,
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
                (get_current_instrument_price(&title_performance.isin).await?
                    * title_performance.units_left)
                    - title_performance.average_unit_price * title_performance.units_left,
            )
        } else {
            dec!(0.0)
        };

        let pl = round_to_decimals(realized_pl + unrealized_pl);

        let performance_item = PositionPl {
            isin: title_performance.isin.clone(),
            name: get_current_instrument_name(&title_performance.isin.clone()).await?,
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
            name: get_current_instrument_name(&simulated_performance.isin.clone()).await?,
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

#[derive(Debug)]
pub struct TradeGroup {
    pub isin: String,
    pub broker: String,
    pub trades: Vec<Trade>,
}

#[derive(Debug, Tabled)]
pub struct TradeGroupPerformance {
    pub isin: String,
    pub broker: String,
    pub units_left: Decimal,
    pub average_unit_price: Decimal,
    pub realized_profit_eur: Decimal,
    pub invested_cash: Decimal,
}

pub fn get_title_performance(
    trade_group: &TradeGroup,
    date_until: DateTime<Utc>,
    stock_split_information: &mut [StockSplit],
) -> TradeGroupPerformance {
    let trade_group_sorted_by_date = &mut <&TradeGroup>::clone(&trade_group).trades.clone();
    trade_group_sorted_by_date.sort_unstable_by_key(|item| (item.date, item.direction.clone()));

    let queue = trade_group_sorted_by_date
        .iter()
        .filter(|item| item.date.timestamp_millis() < date_until.timestamp_millis());

    let mut held_units = dec!(0.0);
    let mut purchase_value = dec!(0.0);
    let mut pnl = dec!(0.0);
    let mut invested_cash = dec!(0.0);

    let queue_len = &queue.clone().count();

    for (i, trade) in queue.enumerate() {
        let split_adjusted_units = get_split_adjusted_units(
            &trade.isin,
            trade.no_units,
            trade.date,
            stock_split_information,
        );

        if trade.direction == "Buy" {
            held_units += split_adjusted_units;
            purchase_value += get_split_adjusted_price_per_unit(
                &trade.isin,
                trade.eur_avg_price_per_unit,
                trade.date,
                stock_split_information,
            ) * split_adjusted_units;
            invested_cash += get_split_adjusted_price_per_unit(
                &trade.isin,
                trade.eur_avg_price_per_unit,
                trade.date,
                stock_split_information,
            ) * split_adjusted_units;
        }
        if trade.direction == "Sell" {
            if held_units == dec!(0) {
                panic!(
                    "ISIN {} has no units in the portfolio at the point of sell event.",
                    trade.isin
                )
            }
            let avg_purchase_price = purchase_value / held_units;
            let actual_sell_price = get_split_adjusted_price_per_unit(
                &trade.isin,
                trade.eur_avg_price_per_unit,
                trade.date,
                stock_split_information,
            );

            let realized_pnl_for_trade =
                (actual_sell_price - avg_purchase_price) * split_adjusted_units;
            pnl += realized_pnl_for_trade;
            held_units -= split_adjusted_units;
            purchase_value -= avg_purchase_price * split_adjusted_units;
            invested_cash -= if !position_size_over_threshold(held_units) && queue_len != &(&i + 1)
            {
                realized_pnl_for_trade + avg_purchase_price * split_adjusted_units
            } else if position_size_over_threshold(held_units) && queue_len == &(&i + 1) {
                realized_pnl_for_trade
            } else {
                dec!(0.0)
            }
        };
    }

    held_units = override_positions_below_threshold(held_units);

    TradeGroupPerformance {
        isin: trade_group.isin.to_string(),
        broker: trade_group.broker.to_string(),
        units_left: held_units,
        average_unit_price: if held_units > dec!(0.0) {
            purchase_value / held_units
        } else {
            dec!(0.0)
        },
        realized_profit_eur: pnl,
        invested_cash,
    }
}

pub fn position_size_over_threshold(no_units: Decimal) -> bool {
    no_units > dec!(0.00000000000001)
}

pub fn override_positions_below_threshold(no_units: Decimal) -> Decimal {
    if position_size_over_threshold(no_units) {
        no_units
    } else {
        dec!(0.0)
    }
}

pub async fn simulate_alternate_purchase(
    trade_group_for_simulation: &TradeGroup,
    date_until: DateTime<Utc>,
    index: Option<&FREDResponse>,
) -> anyhow::Result<Option<TradeGroupPerformance>> {
    if index.is_some() {
        let trade_group_sorted_by_date = &mut <&TradeGroup>::clone(&trade_group_for_simulation)
            .trades
            .clone();

        trade_group_sorted_by_date.sort_unstable_by_key(|item| (item.date, item.direction.clone()));

        let queue_without_overrides = trade_group_sorted_by_date
            .iter()
            .filter(|item| item.date.timestamp_millis() < date_until.timestamp_millis());

        let mut queue_with_overrides: Vec<Trade> = vec![];

        for queue_item_without_overrides in queue_without_overrides.clone() {
            let index_price_during_trade = get_fred_value_for_date(
                index.unwrap(),
                queue_item_without_overrides.date.date_naive(),
            )
            .await?;

            let trade_with_index_overrides = Trade {
                isin: queue_item_without_overrides.isin.to_string(),
                broker: queue_item_without_overrides.broker.to_string(),
                date: queue_item_without_overrides.date,
                no_units: (queue_item_without_overrides.eur_avg_price_per_unit
                    * queue_item_without_overrides.no_units)
                    / index_price_during_trade,
                avg_price_per_unit: index_price_during_trade,
                eur_avg_price_per_unit: index_price_during_trade,
                security_type: queue_item_without_overrides.security_type.to_string(),
                direction: queue_item_without_overrides.direction.to_string(),
                currency_denomination: queue_item_without_overrides
                    .currency_denomination
                    .to_string(),
                date_added: Utc::now(),
                fees: queue_item_without_overrides.fees,
                withholding_tax: dec!(0.9),
                witholding_tax_currency: "EUR".to_string(),
            };
            queue_with_overrides.push(trade_with_index_overrides);
        }

        let mut real_held_units = dec!(0.0);
        let mut held_units = dec!(0.0);
        let mut purchase_value = dec!(0.0);
        let mut pnl = dec!(0.0);
        let mut invested_cash = dec!(0.0);

        let queue_len = &queue_with_overrides.iter().clone().count();

        for (i, trade) in queue_with_overrides.iter().enumerate() {
            if trade.direction == "Buy" {
                held_units += trade.no_units;
                real_held_units += queue_without_overrides.clone().collect_vec()[i].no_units;
                purchase_value += trade.eur_avg_price_per_unit * trade.no_units;
                invested_cash += trade.eur_avg_price_per_unit * trade.no_units;
            }

            if trade.direction == "Sell" {
                let share_of_accrued_position =
                    queue_without_overrides.clone().collect_vec()[i].no_units / real_held_units;

                let normalized_unit_count = share_of_accrued_position * held_units;

                let avg_purchase_price = purchase_value / held_units;
                let actual_sell_price = trade.eur_avg_price_per_unit;

                let realized_pnl_for_trade =
                    (actual_sell_price - avg_purchase_price) * normalized_unit_count;
                pnl += realized_pnl_for_trade;
                held_units -= normalized_unit_count;
                real_held_units -= queue_without_overrides.clone().collect_vec()[i].no_units;
                purchase_value -= avg_purchase_price * normalized_unit_count;
                invested_cash -=
                    if !position_size_over_threshold(held_units) && queue_len != &(&i + 1) {
                        realized_pnl_for_trade + avg_purchase_price * normalized_unit_count
                    } else if position_size_over_threshold(held_units) && queue_len == &(&i + 1) {
                        realized_pnl_for_trade
                    } else {
                        dec!(0.0)
                    }
            };
        }
        held_units = override_positions_below_threshold(held_units);

        Ok(Some(TradeGroupPerformance {
            isin: trade_group_for_simulation.isin.to_string(),
            broker: trade_group_for_simulation.broker.to_string(),
            units_left: held_units,
            average_unit_price: if held_units > dec!(0.0) {
                purchase_value / held_units
            } else {
                dec!(0.0)
            },
            realized_profit_eur: pnl,
            invested_cash,
        }))
    } else {
        Ok(None)
    }
}
