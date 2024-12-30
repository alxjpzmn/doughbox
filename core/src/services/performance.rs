use chrono::{DateTime, Utc};

use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tabled::Tabled;
use typeshare::typeshare;

use crate::database::{
    models::trade::Trade,
    queries::{
        composite::get_all_trades, instrument::get_current_instrument_price,
        stock_split::get_stock_splits,
    },
};
use serde::Serialize;

use super::{
    files::{export_csv, export_json},
    instruments::stock_splits::{
        get_split_adjusted_price_per_unit, get_split_adjusted_units, StockSplit,
    },
    market_data::fred::{fetch_fred_data_set, get_fred_value_for_date, FREDResponse},
    shared::{env::get_env_variable, util::round_to_decimals},
};

#[typeshare]
#[derive(Debug, Serialize)]
pub struct PortfolioPerformance {
    #[typeshare(serialized_as = "number")]
    pub generated_at: i64,
    pub actual: Decimal,
    pub simulated: Decimal,
    pub alpha: Decimal,
    pub position: Vec<PositionPerformance>,
}

// position = trades in the same instrument across multiple brokers
#[typeshare]
#[derive(Debug, Serialize, Clone)]
pub struct PositionPerformance {
    pub isin: String,
    pub name: String,
    pub unrealized: Decimal,
    pub realized: Decimal,
    pub performance: Decimal,
    pub simulated: Decimal,
    pub alpha: Decimal,
    pub invested_amount: Decimal,
    pub total_return: Decimal,
}

#[derive(Debug, Clone)]
pub struct TradePerformance {
    pub isin: String,
    pub name: String,
    pub broker: String,
    pub unrealized: Decimal,
    pub realized: Decimal,
    pub performance: Decimal,
    pub invested_amount: Decimal,
    pub total_return: Decimal,
}

// trade group = trades in the same instrument at the same broker
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
    pub inventory: Decimal,
    pub unit_price: Decimal,
    pub realized: Decimal,
    pub invested_amount: Decimal,
}

pub async fn get_performance() -> anyhow::Result<PortfolioPerformance> {
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

    let mut trade_performance: Vec<TradePerformance> = vec![];

    for title_performance in title_performances {
        let realized = round_to_decimals(title_performance.realized);
        let unrealized = if title_performance.inventory > dec!(0.0) {
            round_to_decimals(
                (get_current_instrument_price(&title_performance.isin).await?
                    * title_performance.inventory)
                    - title_performance.unit_price * title_performance.inventory,
            )
        } else {
            dec!(0.0)
        };

        let performance = round_to_decimals(realized + unrealized);

        let performance_item = TradePerformance {
            isin: title_performance.isin.clone(),
            name: title_performance.isin.clone(),
            broker: title_performance.broker,
            unrealized,
            realized,
            performance,
            total_return: round_to_decimals(
                (performance / title_performance.invested_amount) * dec!(100.0),
            ),
            invested_amount: title_performance.invested_amount,
        };
        trade_performance.push(performance_item);
    }

    trade_performance.sort_by(|a, b| a.performance.partial_cmp(&b.performance).unwrap());

    let mut simulated_trade_performance: Vec<TradePerformance> = vec![];

    for simulated_performance in simulated_sp500_title_performances {
        let realized = round_to_decimals(simulated_performance.realized);
        let unrealized = if simulated_performance.inventory > dec!(0.0) {
            round_to_decimals(
                (get_fred_value_for_date(index_values.as_ref().unwrap(), Utc::now().date_naive())
                    .await?
                    * simulated_performance.inventory)
                    - simulated_performance.unit_price * simulated_performance.inventory,
            )
        } else {
            dec!(0.0)
        };

        let performance = round_to_decimals(realized + unrealized);

        let trade_performance = TradePerformance {
            isin: simulated_performance.isin.clone(),
            name: simulated_performance.isin.clone(),
            broker: simulated_performance.broker,
            unrealized,
            realized,
            performance,
            total_return: round_to_decimals(
                (performance / simulated_performance.invested_amount) * dec!(100.0),
            ),
            invested_amount: simulated_performance.invested_amount,
        };
        simulated_trade_performance.push(trade_performance);
    }

    simulated_trade_performance.sort_by(|a, b| a.performance.partial_cmp(&b.performance).unwrap());

    let total_performance = &trade_performance
        .clone()
        .into_iter()
        .fold(dec!(0.0), |acc, item| acc + item.performance);

    let total_simulated_performance = &simulated_trade_performance
        .clone()
        .into_iter()
        .fold(dec!(0.0), |acc, item| acc + item.performance);

    let mut merged_positions: Vec<PositionPerformance> = vec![];

    for item in trade_performance.iter() {
        let simulated_item = match fred_token_set {
            true => simulated_trade_performance
                .iter()
                .find(|simulated_item| {
                    simulated_item.broker == item.broker && simulated_item.isin == item.isin
                })
                .unwrap(),
            false => &TradePerformance {
                isin: "".to_string(),
                name: "".to_string(),
                broker: "".to_string(),
                unrealized: dec!(0),
                realized: dec!(0),
                performance: dec!(0),
                total_return: dec!(0),
                invested_amount: dec!(0),
            },
        };

        let merged_position_pl = PositionPerformance {
            isin: item.isin.clone(),
            name: item.name.clone(),
            unrealized: round_to_decimals(item.unrealized),
            realized: round_to_decimals(item.realized),
            performance: round_to_decimals(item.performance),
            simulated: round_to_decimals(simulated_item.performance),
            alpha: round_to_decimals(item.performance - simulated_item.performance),
            total_return: round_to_decimals(item.total_return),
            invested_amount: item.invested_amount,
        };
        merged_positions.push(merged_position_pl);
    }

    let performance_overview = PortfolioPerformance {
        generated_at: Utc::now().timestamp(),
        actual: round_to_decimals(*total_performance),
        simulated: round_to_decimals(*total_simulated_performance),
        alpha: round_to_decimals(total_performance - total_simulated_performance),
        position: merged_positions.clone(),
    };

    export_csv(&merged_positions, "performance")?;
    export_json(&performance_overview, "performance")?;

    Ok(performance_overview)
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

    let mut inventory = dec!(0.0);
    let mut purchase_value = dec!(0.0);
    let mut realized = dec!(0.0);
    let mut invested_amount = dec!(0.0);

    let queue_len = &queue.clone().count();

    for (i, trade) in queue.enumerate() {
        let split_adjusted_units = get_split_adjusted_units(
            &trade.isin,
            trade.no_units,
            trade.date,
            stock_split_information,
        );

        if trade.direction == "Buy" {
            inventory += split_adjusted_units;
            purchase_value += get_split_adjusted_price_per_unit(
                &trade.isin,
                trade.eur_avg_price_per_unit,
                trade.date,
                stock_split_information,
            ) * split_adjusted_units;
            invested_amount += get_split_adjusted_price_per_unit(
                &trade.isin,
                trade.eur_avg_price_per_unit,
                trade.date,
                stock_split_information,
            ) * split_adjusted_units;
        }
        if trade.direction == "Sell" {
            if inventory == dec!(0) {
                panic!(
                    "ISIN {} has no units in the portfolio at the point of sell event.",
                    trade.isin
                )
            }
            let avg_purchase_price = purchase_value / inventory;
            let actual_sell_price = get_split_adjusted_price_per_unit(
                &trade.isin,
                trade.eur_avg_price_per_unit,
                trade.date,
                stock_split_information,
            );

            let realized_pnl_for_trade =
                (actual_sell_price - avg_purchase_price) * split_adjusted_units;
            realized += realized_pnl_for_trade;
            inventory -= split_adjusted_units;
            purchase_value -= avg_purchase_price * split_adjusted_units;
            invested_amount -=
                if !is_position_size_over_threshold(inventory) && queue_len != &(&i + 1) {
                    realized_pnl_for_trade + avg_purchase_price * split_adjusted_units
                } else if is_position_size_over_threshold(inventory) && queue_len == &(&i + 1) {
                    realized_pnl_for_trade
                } else {
                    dec!(0.0)
                }
        };
    }

    inventory = override_positions_below_threshold(inventory);

    TradeGroupPerformance {
        isin: trade_group.isin.to_string(),
        broker: trade_group.broker.to_string(),
        inventory,
        unit_price: if inventory > dec!(0.0) {
            purchase_value / inventory
        } else {
            dec!(0.0)
        },
        realized,
        invested_amount,
    }
}

pub fn is_position_size_over_threshold(no_units: Decimal) -> bool {
    no_units > dec!(0.00000000000001)
}

pub fn override_positions_below_threshold(no_units: Decimal) -> Decimal {
    if is_position_size_over_threshold(no_units) {
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

        let mut inventory = dec!(0.0);
        let mut real_held_units = dec!(0.0);
        let mut purchase_value = dec!(0.0);
        let mut realized = dec!(0.0);
        let mut invested_amount = dec!(0.0);

        let queue_len = &queue_with_overrides.iter().clone().count();

        for (i, trade) in queue_with_overrides.iter().enumerate() {
            if trade.direction == "Buy" {
                inventory += trade.no_units;
                real_held_units += queue_without_overrides.clone().collect_vec()[i].no_units;
                purchase_value += trade.eur_avg_price_per_unit * trade.no_units;
                invested_amount += trade.eur_avg_price_per_unit * trade.no_units;
            }

            if trade.direction == "Sell" {
                let share_of_accrued_position =
                    queue_without_overrides.clone().collect_vec()[i].no_units / real_held_units;

                let normalized_unit_count = share_of_accrued_position * inventory;

                let avg_purchase_price = purchase_value / inventory;
                let actual_sell_price = trade.eur_avg_price_per_unit;

                let realized_for_trade =
                    (actual_sell_price - avg_purchase_price) * normalized_unit_count;
                realized += realized_for_trade;
                inventory -= normalized_unit_count;
                real_held_units -= queue_without_overrides.clone().collect_vec()[i].no_units;
                purchase_value -= avg_purchase_price * normalized_unit_count;
                invested_amount -=
                    if !is_position_size_over_threshold(inventory) && queue_len != &(&i + 1) {
                        realized_for_trade + avg_purchase_price * normalized_unit_count
                    } else if is_position_size_over_threshold(inventory) && queue_len == &(&i + 1) {
                        realized_for_trade
                    } else {
                        dec!(0.0)
                    }
            };
        }
        inventory = override_positions_below_threshold(inventory);

        Ok(Some(TradeGroupPerformance {
            isin: trade_group_for_simulation.isin.to_string(),
            broker: trade_group_for_simulation.broker.to_string(),
            inventory,
            unit_price: if inventory > dec!(0.0) {
                purchase_value / inventory
            } else {
                dec!(0.0)
            },
            realized,
            invested_amount,
        }))
    } else {
        Ok(None)
    }
}
