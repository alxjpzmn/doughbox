use chrono::{DateTime, Utc};

use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tabled::Tabled;

use super::{
    db_helpers::{StockSplit, Trade},
    market_data_helpers::{
        get_fred_value_for_date, get_split_adjusted_price_per_unit, get_split_adjusted_units,
        FREDResponse,
    },
};

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

pub fn round_to_decimals(input: Decimal) -> Decimal {
    input.round_dp(2)
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
