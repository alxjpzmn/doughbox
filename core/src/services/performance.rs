use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tabled::Tabled;
use typeshare::typeshare;

use crate::database::{
    models::{dividend::Dividend, trade::Trade},
    queries::{
        composite::get_all_trades,
        dividend::get_dividends,
        instrument::{batch_get_instrument_names, batch_get_instrument_prices},
        listing_change::get_listing_changes,
        stock_split::get_stock_splits,
        QueryFilter,
    },
};
use crate::services::instruments::identifiers::get_changed_identifier;
use serde::Serialize;
use utoipa::ToSchema;

use super::{
    files::{export_csv, export_json},
    instruments::stock_splits::{get_split_adjusted_units, StockSplit},
    market_data::fred::{fetch_fred_data_set, get_fred_value_for_date, FREDResponse},
    shared::{env::get_env_variable, util::round_to_decimals},
};

#[typeshare]
#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioPerformance {
    #[typeshare(serialized_as = "number")]
    pub generated_at: i64,
    pub actual: Decimal,
    pub simulated: Option<Decimal>,
    pub alpha: Option<Decimal>,
    pub position: Vec<PositionPerformance>,
}

// position = trades in the same instrument across multiple brokers
#[typeshare]
#[derive(Debug, Serialize, Clone, ToSchema)]
pub struct PositionPerformance {
    pub isin: String,
    pub name: String,
    pub unrealized: Decimal,
    pub realized: Decimal,
    pub income: Decimal,
    pub performance: Decimal,
    pub simulated: Option<Decimal>,
    pub alpha: Option<Decimal>,
    pub invested_amount: Decimal,
    pub total_return: Decimal,
}


#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BuyIn {
    pub isin: String,
    pub name: String,
    pub broker: String,
    pub units: Decimal,
    pub average_buy_in: Decimal,
    pub invested_amount: Decimal,
    pub current_price: Decimal,
    pub current_value: Decimal,
    pub unrealized: Decimal,
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

#[typeshare]
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SecurityHistoryGap {
    pub isin: String,
    pub broker: String,
    pub date: DateTime<Utc>,
    pub available_units: Decimal,
    pub sold_units: Decimal,
}

impl std::fmt::Display for SecurityHistoryGap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter,
            "Incomplete acquisition history for {} at {} on {}: sold {} shares, but recorded inventory is {}. Import or reconcile the missing acquisitions or transfers before calculating returns.",
            self.isin, self.broker, self.date, self.sold_units, self.available_units)
    }
}

impl std::error::Error for SecurityHistoryGap {}

/// Raw economic costs and recorded gross EUR income, aggregated across brokers.
#[derive(Debug)]
pub(crate) struct SecurityCost {
    pub(crate) isin: String,
    pub(crate) units: Decimal,
    pub(crate) realized: Decimal,
    pub(crate) invested_amount: Decimal,
    pub(crate) remaining_cost: Decimal,
    pub(crate) income: Decimal,
    pub(crate) history_gaps: Vec<SecurityHistoryGap>,
}

async fn load_trade_groups(
    isin_filter: Option<&str>,
    broker_filter: Option<&str>,
) -> anyhow::Result<Vec<TradeGroup>> {
    let trades = get_all_trades(None).await?;
    let listing_changes = get_listing_changes().await?;
    let mut groups = std::collections::BTreeMap::<(String, String), Vec<Trade>>::new();
    for mut trade in trades {
        trade.isin = get_changed_identifier(&trade.isin, listing_changes.clone());
        if isin_filter.is_some_and(|isin| trade.isin != isin)
            || broker_filter.is_some_and(|broker| trade.broker != broker)
        {
            continue;
        }
        groups.entry((trade.broker.clone(), trade.isin.clone())).or_default().push(trade);
    }
    Ok(groups.into_iter().map(|((broker, isin), trades)| TradeGroup {
        broker, isin, trades,
    }).collect())
}

pub async fn get_performance() -> anyhow::Result<PortfolioPerformance> {
    let performance_overview = compute_performance(None, None).await?;
    export_csv(&performance_overview.position, "performance")?;
    export_json(&performance_overview, "performance")?;
    Ok(performance_overview)
}

pub async fn compute_performance(
    isin_filter: Option<&str>,
    broker_filter: Option<&str>,
) -> anyhow::Result<PortfolioPerformance> {
    let as_of = Utc::now();
    let (groups, costs, mut splits) =
        load_security_accounting(isin_filter, broker_filter, as_of).await?;
    if let Some(gap) = costs.iter().find_map(|cost| cost.history_gaps.first()) {
        return Err(gap.clone().into());
    }
    let isins: Vec<_> = costs.iter().map(|cost| cost.isin.clone()).collect();
    let prices = batch_get_instrument_prices(&isins).await?;
    let names = batch_get_instrument_names(&isins).await?;
    let index = if get_env_variable("FRED_TOKEN").is_some() {
        Some(fetch_fred_data_set("SP500").await?)
    } else {
        None
    };
    let current_index = match &index {
        Some(index) => get_fred_value_for_date(index, as_of.date_naive()).await?,
        None => dec!(0),
    };
    // The benchmark is a nominal SP500 price-index proxy, not a dividend-reinvestment model.
    let mut simulated_by_isin = HashMap::<&str, Decimal>::new();
    for group in &groups {
        if let Some(cost) = simulate_alternate_purchase(group, as_of, index.as_ref(), &mut splits).await? {
            *simulated_by_isin.entry(&group.isin).or_default() +=
                cost.realized + (current_index - cost.unit_price) * cost.inventory;
        }
    }
    let mut positions = Vec::with_capacity(costs.len());
    for ((cost, price), name) in costs.into_iter().zip(prices).zip(names) {
        let simulated = index.as_ref().map(|_| {
            simulated_by_isin.get(cost.isin.as_str()).copied().unwrap_or(dec!(0))
        });
        positions.push(position_from_cost(cost, name, price, simulated)?);
    }
    let (mut actual, mut simulated) = (dec!(0), dec!(0));
    for position in &mut positions {
        actual += position.performance;
        simulated += position.simulated.unwrap_or(dec!(0));
        position.realized = round_to_decimals(position.realized);
        position.income = round_to_decimals(position.income);
        position.unrealized = round_to_decimals(position.unrealized);
        position.performance = round_to_decimals(position.performance);
        position.simulated = position.simulated.map(round_to_decimals);
        position.invested_amount = round_to_decimals(position.invested_amount);
    }
    positions.sort_by_key(|position| position.performance);
    Ok(PortfolioPerformance {
        generated_at: as_of.timestamp(),
        actual: round_to_decimals(actual),
        simulated: index.as_ref().map(|_| round_to_decimals(simulated)),
        alpha: index.as_ref().map(|_| round_to_decimals(actual - simulated)),
        position: positions,
    })
}

fn position_from_cost(
    cost: SecurityCost,
    name: String,
    price: Decimal,
    simulated: Option<Decimal>,
) -> anyhow::Result<PositionPerformance> {
    anyhow::ensure!(cost.units <= dec!(0) || price > dec!(0),
        "No usable valuation price for {}", cost.isin);
    let unrealized = price * cost.units - cost.remaining_cost;
    let performance = cost.realized + unrealized + cost.income;
    Ok(PositionPerformance {
        isin: cost.isin,
        name,
        unrealized,
        realized: cost.realized,
        income: cost.income,
        performance,
        simulated,
        alpha: simulated.map(|value| round_to_decimals(performance - value)),
        invested_amount: cost.invested_amount,
        total_return: if cost.invested_amount > dec!(0) {
            round_to_decimals(performance / cost.invested_amount * dec!(100))
        } else { dec!(0) },
    })
}

pub async fn get_buy_ins(
    isin_filter: Option<&str>,
    broker_filter: Option<&str>,
) -> anyhow::Result<Vec<BuyIn>> {
    let grouped_trades = load_trade_groups(isin_filter, broker_filter).await?;
    let isins: Vec<_> = grouped_trades
        .iter()
        .map(|group| group.isin.clone())
        .collect();
    let prices = batch_get_instrument_prices(&isins).await?;
    let names = batch_get_instrument_names(&isins).await?;
    let price_map: HashMap<_, _> = isins.iter().zip(prices.iter()).collect();
    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();
    let mut stock_split_information = get_stock_splits().await?;

    let mut buy_ins = Vec::new();
    for group in grouped_trades {
        let performance = get_title_performance(&group, Utc::now(), &mut stock_split_information)?;
        let current_price = **price_map.get(&group.isin).unwrap_or(&&dec!(0.0));
        let units = performance.inventory;
        anyhow::ensure!(
            units <= dec!(0) || current_price > dec!(0),
            "No usable valuation price for {}",
            group.isin
        );
        let current_value = current_price * units;
        let unrealized = if units > dec!(0.0) {
            current_value - performance.unit_price * units
        } else {
            dec!(0.0)
        };
        buy_ins.push(BuyIn {
            isin: group.isin.clone(),
            name: name_map
                .get(&group.isin)
                .unwrap_or(&&group.isin.to_string())
                .to_string(),
            broker: group.broker,
            units,
            average_buy_in: round_to_decimals(performance.unit_price),
            invested_amount: round_to_decimals(performance.invested_amount),
            current_price: round_to_decimals(current_price),
            current_value: round_to_decimals(current_value),
            unrealized: round_to_decimals(unrealized),
        });
    }
    buy_ins.sort_by(|a, b| a.isin.cmp(&b.isin).then(a.broker.cmp(&b.broker)));
    Ok(buy_ins)
}

/// Economic EUR WAC, including EUR transaction fees; not the Austrian tax basis.
pub fn get_title_performance(
    trade_group: &TradeGroup,
    date_until: DateTime<Utc>,
    stock_split_information: &mut [StockSplit],
) -> anyhow::Result<TradeGroupPerformance> {
    let mut trades: Vec<_> = trade_group.trades.iter()
        .filter(|trade| trade.date <= date_until).collect();
    trades.sort_by_key(|trade| (trade.date, &trade.direction, trade.date_added));
    let mut inventory = dec!(0);
    let mut purchase_value = dec!(0);
    let mut realized = dec!(0);
    let mut invested_amount = dec!(0);

    for trade in trades {
        let units = get_split_adjusted_units(
            &trade.isin, trade.units, trade.date, stock_split_information,
        );
        let value = trade.eur_avg_price_per_unit * trade.units;
        match trade.direction.as_str() {
            "Buy" => {
                inventory += units;
                purchase_value += value + trade.fees;
                invested_amount += value + trade.fees;
            }
            "Sell" => {
                validate_sale_inventory(trade, units, inventory)?;
                let removed_cost = purchase_value * units / inventory;
                realized += value - trade.fees - removed_cost;
                purchase_value -= removed_cost;
                inventory -= units;
                if !is_position_size_over_threshold(inventory) {
                    // Preserve the cash-flow identity when decimal dust is closed.
                    realized -= purchase_value;
                    inventory = dec!(0);
                    purchase_value = dec!(0);
                }
            }
            direction => anyhow::bail!("Unknown trade direction: {direction}"),
        }
    }
    Ok(TradeGroupPerformance {
        isin: trade_group.isin.clone(),
        broker: trade_group.broker.clone(),
        inventory,
        unit_price: if inventory > dec!(0) { purchase_value / inventory } else { dec!(0) },
        realized,
        invested_amount,
    })
}

fn validate_sale_inventory(trade: &Trade, units: Decimal, inventory: Decimal) -> anyhow::Result<()> {
    anyhow::ensure!(units > dec!(0), "Non-positive sale quantity for {}", trade.isin);
    if units > inventory {
        return Err(SecurityHistoryGap {
            isin: trade.isin.clone(),
            broker: trade.broker.clone(),
            date: trade.date,
            available_units: inventory,
            sold_units: units,
        }.into());
    }
    Ok(())
}

/// Per-ISIN costs for portfolio snapshots, without prices, rounding or benchmark IO.
pub(crate) async fn security_costs(as_of: DateTime<Utc>) -> anyhow::Result<Vec<SecurityCost>> {
    let (_, costs, _) = load_security_accounting(None, None, as_of).await?;
    Ok(costs)
}

async fn load_security_accounting(
    isin_filter: Option<&str>,
    broker_filter: Option<&str>,
    as_of: DateTime<Utc>,
) -> anyhow::Result<(Vec<TradeGroup>, Vec<SecurityCost>, Vec<StockSplit>)> {
    let groups = load_trade_groups(isin_filter, broker_filter).await?;
    // get_dividends uses the same listing-change normalization as load_trade_groups.
    let dividends = get_dividends(QueryFilter {
        isin: isin_filter.map(str::to_owned),
        broker: broker_filter.map(str::to_owned),
        until_date: Some(as_of),
        ..QueryFilter::default()
    }).await?;
    let mut splits = get_stock_splits().await?;
    let costs = security_costs_from_records(
        &groups, &dividends, isin_filter, broker_filter, as_of, &mut splits,
    )?;
    Ok((groups, costs, splits))
}

/// Inputs carry canonical ISINs from the shared trade/dividend query normalization.
fn security_costs_from_records(
    groups: &[TradeGroup],
    dividends: &[Dividend],
    isin_filter: Option<&str>,
    broker_filter: Option<&str>,
    as_of: DateTime<Utc>,
    splits: &mut [StockSplit],
) -> anyhow::Result<Vec<SecurityCost>> {
    let mut costs = BTreeMap::new();
    for group in groups {
        if isin_filter.is_some_and(|isin| group.isin != isin)
            || broker_filter.is_some_and(|broker| group.broker != broker)
        {
            continue;
        }
        let cost = costs.entry(group.isin.clone()).or_insert_with(|| empty_security_cost(&group.isin));
        match get_title_performance(group, as_of, splits) {
            Ok(performance) => {
                cost.units += performance.inventory;
                cost.realized += performance.realized;
                cost.invested_amount += performance.invested_amount;
                cost.remaining_cost += performance.inventory * performance.unit_price;
            }
            Err(error) => match error.downcast::<SecurityHistoryGap>() {
                Ok(gap) => {
                    cost.history_gaps.push(gap);
                    cost.invested_amount += group.trades.iter()
                        .filter(|trade| trade.date <= as_of && trade.direction == "Buy")
                        .map(|trade| trade.eur_avg_price_per_unit * trade.units + trade.fees)
                        .sum::<Decimal>();
                }
                Err(error) => return Err(error),
            },
        }
    }
    // A canonical holding's market value includes every broker. If any pool is
    // incomplete, none of that holding's trading P&L can use an invented basis.
    for cost in costs.values_mut().filter(|cost| !cost.history_gaps.is_empty()) {
        cost.units = dec!(0);
        cost.realized = dec!(0);
        cost.remaining_cost = dec!(0);
    }
    for dividend in dividends {
        if dividend.date > as_of
            || isin_filter.is_some_and(|isin| dividend.isin != isin)
            || broker_filter.is_some_and(|broker| dividend.broker != broker)
        {
            continue;
        }
        // Gross recorded EUR amounts include signed corrections, never withholding or estimates.
        costs.entry(dividend.isin.clone())
            .or_insert_with(|| empty_security_cost(&dividend.isin)).income += dividend.amount_eur;
    }
    Ok(costs.into_values().collect())
}

fn empty_security_cost(isin: &str) -> SecurityCost {
    SecurityCost {
        isin: isin.to_owned(),
        units: dec!(0),
        realized: dec!(0),
        invested_amount: dec!(0),
        remaining_cost: dec!(0),
        income: dec!(0),
        history_gaps: Vec::new(),
    }
}

pub fn is_position_size_over_threshold(units: Decimal) -> bool {
    units > dec!(0.00000000000001)
}


pub async fn simulate_alternate_purchase(
    group: &TradeGroup,
    date_until: DateTime<Utc>,
    index: Option<&FREDResponse>,
    splits: &mut [StockSplit],
) -> anyhow::Result<Option<TradeGroupPerformance>> {
    let Some(index) = index else { return Ok(None) };
    let mut trades: Vec<_> = group.trades.iter().filter(|trade| trade.date <= date_until).collect();
    trades.sort_by_key(|trade| (trade.date, &trade.direction, trade.date_added));
    let (mut inventory, mut real_units, mut cost, mut realized, mut invested) =
        (dec!(0), dec!(0), dec!(0), dec!(0), dec!(0));
    for trade in trades {
        let price = get_fred_value_for_date(index, trade.date.date_naive()).await?;
        anyhow::ensure!(price > dec!(0), "Invalid benchmark price on {}", trade.date);
        let units = get_split_adjusted_units(&trade.isin, trade.units, trade.date, splits);
        match trade.direction.as_str() {
            "Buy" => {
                let spend = trade.eur_avg_price_per_unit * trade.units + trade.fees;
                inventory += spend / price;
                real_units += units;
                cost += spend;
                invested += spend;
            }
            "Sell" => {
                validate_sale_inventory(trade, units, real_units)?;
                let fraction = units / real_units;
                let sold_units = inventory * fraction;
                let removed_cost = cost * fraction;
                realized += sold_units * price - removed_cost;
                inventory -= sold_units;
                real_units -= units;
                cost -= removed_cost;
                if !is_position_size_over_threshold(real_units) {
                    realized -= cost;
                    inventory = dec!(0);
                    cost = dec!(0);
                    real_units = dec!(0);
                }
            }
            direction => anyhow::bail!("Unknown trade direction: {direction}"),
        }
    }
    Ok(Some(TradeGroupPerformance {
        isin: group.isin.clone(), broker: group.broker.clone(), inventory,
        unit_price: if inventory > dec!(0) { cost / inventory } else { dec!(0) },
        realized, invested_amount: invested,
    }))
}

#[cfg(test)]
mod threshold_tests {
    use super::*;

    use chrono::TimeZone;

    fn dividend(day: u32, isin: &str, broker: &str, amount_eur: Decimal) -> Dividend {
        Dividend {
            isin: isin.into(),
            date: Utc.with_ymd_and_hms(2025, 1, day, 0, 0, 0).unwrap(),
            amount: amount_eur * dec!(10),
            broker: broker.into(),
            currency: "USD".into(),
            amount_eur,
            withholding_tax: dec!(999),
            withholding_tax_currency: "USD".into(),
        }
    }

    #[test]
    fn recorded_income_respects_canonical_group_broker_cutoff_and_gross_fee_basis() {
        let isin = "AT0000000001";
        let mut other_trade = trade(1, "Buy", dec!(2), dec!(100), dec!(2));
        other_trade.broker = "Other".into();
        // Both sources carry canonical identifiers, as supplied by the shared query normalization.
        let groups = vec![
            TradeGroup {
                isin: isin.into(), broker: "Test".into(),
                trades: vec![
                    trade(1, "Buy", dec!(10), dec!(100), dec!(10)),
                    trade(2, "Sell", dec!(4), dec!(120), dec!(4)),
                ],
            },
            TradeGroup {
                isin: isin.into(), broker: "Other".into(), trades: vec![other_trade],
            },
        ];
        let dividends = vec![
            dividend(3, isin, "Test", dec!(40)),
            dividend(5, isin, "Test", dec!(-5)),
            dividend(6, isin, "Test", dec!(1000)),
            dividend(3, isin, "Other", dec!(9)),
            dividend(4, "income-only", "Test", dec!(7)),
        ];
        let as_of = Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap();
        let mut all = security_costs_from_records(
            &groups, &dividends, None, None, as_of, &mut [],
        ).unwrap();
        assert_eq!(all.len(), 2);
        let cost = all.remove(0);
        assert_eq!(cost.isin, isin);
        assert_eq!(cost.units, dec!(8));
        assert_eq!(cost.remaining_cost, dec!(808));
        assert_eq!(cost.invested_amount, dec!(1212));
        assert_eq!(cost.realized, dec!(72));
        assert_eq!(cost.income, dec!(44));
        let position = position_from_cost(cost, "Test".into(), dec!(110), Some(dec!(20))).unwrap();
        assert_eq!(position.unrealized, dec!(72));
        assert_eq!(position.performance, dec!(188));
        assert_eq!(position.alpha, Some(dec!(168)));
        assert_eq!(position.total_return, dec!(15.51));

        let mut filtered = security_costs_from_records(
            &groups, &dividends, Some(isin), Some("Test"), as_of, &mut [],
        ).unwrap();
        assert_eq!(filtered.len(), 1);
        let cost = filtered.remove(0);
        assert_eq!(cost.units, dec!(6));
        assert_eq!(cost.remaining_cost, dec!(606));
        assert_eq!(cost.invested_amount, dec!(1010));
        assert_eq!(cost.realized, dec!(72));
        assert_eq!(cost.income, dec!(35));
        let position = position_from_cost(cost, "Test".into(), dec!(110), Some(dec!(20))).unwrap();
        assert_eq!(position.unrealized, dec!(54));
        assert_eq!(position.performance, dec!(161));
        assert_eq!(position.alpha, Some(dec!(141)));
        assert_eq!(position.total_return, dec!(15.94));
    }

    #[test]
    fn closed_and_income_only_instruments_keep_income_without_requiring_a_quote() {
        let isin = "AT0000000001";
        let groups = vec![TradeGroup {
            isin: isin.into(), broker: "Test".into(),
            trades: vec![
                trade(1, "Buy", dec!(1), dec!(20), dec!(2)),
                trade(2, "Sell", dec!(1), dec!(30), dec!(1)),
            ],
        }];
        let dividends = vec![
            dividend(3, isin, "Test", dec!(5)),
            dividend(4, "income-only", "Test", dec!(7)),
            dividend(5, "income-only", "Test", dec!(-2)),
        ];
        let as_of = Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap();
        let costs = security_costs_from_records(
            &groups, &dividends, None, None, as_of, &mut [],
        ).unwrap();
        let positions: Vec<_> = costs.into_iter()
            .map(|cost| position_from_cost(cost, "Test".into(), dec!(0), None).unwrap())
            .collect();
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].realized, dec!(7));
        assert_eq!(positions[0].unrealized, dec!(0));
        assert_eq!(positions[0].income, dec!(5));
        assert_eq!(positions[0].performance, dec!(12));
        assert_eq!(positions[0].invested_amount, dec!(22));
        assert_eq!(positions[0].total_return, dec!(54.55));
        assert_eq!(positions[1].realized, dec!(0));
        assert_eq!(positions[1].unrealized, dec!(0));
        assert_eq!(positions[1].income, dec!(5));
        assert_eq!(positions[1].performance, dec!(5));
        assert_eq!(positions[1].invested_amount, dec!(0));
        assert_eq!(positions[1].total_return, dec!(0));
        assert_eq!(positions[1].simulated, None);
        assert_eq!(positions.iter().map(|position| position.performance).sum::<Decimal>(), dec!(17));
    }

    #[test]
    fn incomplete_history_keeps_income_but_never_invents_a_zero_cost_gain() {
        let isin = "AT0000000001";
        let incomplete = TradeGroup {
            isin: isin.into(), broker: "Test".into(),
            trades: vec![
                trade(1, "Buy", dec!(2), dec!(100), dec!(2)),
                trade(2, "Sell", dec!(3), dec!(120), dec!(3)),
            ],
        };
        let error = get_title_performance(&incomplete, Utc::now(), &mut []).unwrap_err();
        let gap = error.downcast_ref::<SecurityHistoryGap>().unwrap();
        assert_eq!(gap.available_units, dec!(2));
        assert_eq!(gap.sold_units, dec!(3));

        let mut other_broker = trade(1, "Buy", dec!(4), dec!(50), dec!(1));
        other_broker.broker = "Other".into();
        let mut valid_buy = trade(1, "Buy", dec!(2), dec!(80), dec!(2));
        valid_buy.isin = "OTHER".into();
        let mut valid_sale = trade(2, "Sell", dec!(1), dec!(90), dec!(1));
        valid_sale.isin = "OTHER".into();
        let groups = vec![
            incomplete,
            TradeGroup { isin: isin.into(), broker: "Other".into(), trades: vec![other_broker] },
            TradeGroup { isin: "OTHER".into(), broker: "Test".into(), trades: vec![valid_buy, valid_sale] },
        ];
        let costs = security_costs_from_records(
            &groups, &[dividend(3, isin, "Test", dec!(5))],
            None, None, Utc::now(), &mut [],
        ).unwrap();
        let incomplete_cost = costs.iter().find(|cost| cost.isin == isin).unwrap();
        assert_eq!(incomplete_cost.history_gaps.len(), 1);
        assert_eq!(incomplete_cost.units, dec!(0));
        assert_eq!(incomplete_cost.remaining_cost, dec!(0));
        assert_eq!(incomplete_cost.realized, dec!(0));
        assert_eq!(incomplete_cost.invested_amount, dec!(403));
        assert_eq!(incomplete_cost.income, dec!(5));
        let valid = costs.iter().find(|cost| cost.isin == "OTHER").unwrap();
        assert!(valid.history_gaps.is_empty());
        assert_eq!(valid.units, dec!(1));
        assert_eq!(valid.remaining_cost, dec!(81));
        assert_eq!(valid.realized, dec!(8));
        assert_eq!(valid.invested_amount, dec!(162));
    }

    fn trade(day: u32, direction: &str, units: Decimal, price: Decimal, fees: Decimal) -> Trade {
        Trade {
            broker: "Test".into(),
            isin: "AT0000000001".into(),
            date: Utc.with_ymd_and_hms(2025, 1, day, 0, 0, 0).unwrap(),
            date_added: Utc.with_ymd_and_hms(2025, 1, day, 0, 0, 0).unwrap(),
            units,
            avg_price_per_unit: price,
            eur_avg_price_per_unit: price,
            direction: direction.into(),
            security_type: "Equity".into(),
            currency: "EUR".into(),
            fees,
            withholding_tax: dec!(0),
            withholding_tax_currency: "EUR".into(),
        }
    }

    #[test]
    fn partial_sales_do_not_reduce_gross_return_denominator() {
        let group = TradeGroup {
            isin: "AT0000000001".into(),
            broker: "Test".into(),
            trades: vec![
                trade(1, "Buy", dec!(10), dec!(100), dec!(0)),
                trade(2, "Buy", dec!(5), dec!(200), dec!(0)),
                trade(3, "Sell", dec!(3), dec!(150), dec!(0)),
            ],
        };
        let result = get_title_performance(
            &group, Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap(), &mut [],
        ).unwrap();
        assert_eq!(result.invested_amount, dec!(2000));
        assert!((result.realized - dec!(50)).abs() < dec!(0.00000001));
    }

    #[test]
    fn fees_and_closed_reopened_positions_preserve_cash_flow_identity() {
        let group = TradeGroup {
            isin: "AT0000000001".into(), broker: "Test".into(),
            trades: vec![
                trade(1, "Buy", dec!(10), dec!(100), dec!(10)),
                trade(2, "Sell", dec!(10), dec!(120), dec!(5)),
                trade(3, "Buy", dec!(2), dec!(80), dec!(2)),
                trade(4, "Sell", dec!(1), dec!(100), dec!(1)),
            ],
        };
        let result = get_title_performance(
            &group, Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap(), &mut [],
        ).unwrap();
        assert_eq!(result.invested_amount, dec!(1172));
        assert_eq!(result.inventory, dec!(1));
        assert_eq!(result.unit_price, dec!(81));
        assert_eq!(result.realized, dec!(203));
        let market_value = dec!(110);
        assert_eq!(result.realized + market_value - result.unit_price,
            market_value + dec!(1195) + dec!(99) - result.invested_amount);
    }

    #[test]
    fn incomplete_sell_history_is_an_error_not_fabricated_performance() {
        let group = TradeGroup {
            isin: "AT0000000001".into(), broker: "Test".into(),
            trades: vec![trade(1, "Sell", dec!(1), dec!(100), dec!(0))],
        };
        assert!(get_title_performance(&group, Utc::now(), &mut []).is_err());
    }

    #[tokio::test]
    async fn benchmark_sales_use_split_adjusted_held_fraction_and_keep_gross_spend() {
        let group = TradeGroup {
            isin: "AT0000000001".into(), broker: "Test".into(),
            trades: vec![
                trade(1, "Buy", dec!(10), dec!(100), dec!(10)),
                trade(3, "Sell", dec!(10), dec!(60), dec!(0)),
            ],
        };
        let index: FREDResponse = serde_json::from_str(r#"{"observations":[
            {"date":"2025-01-01","value":"100"},
            {"date":"2025-01-03","value":"120"}
        ]}"#).unwrap();
        let mut splits = [StockSplit {
            id: "split".into(), isin: group.isin.clone(),
            ex_date: Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
            from_factor: dec!(1), to_factor: dec!(2),
        }];
        let result = simulate_alternate_purchase(
            &group, Utc::now(), Some(&index), &mut splits,
        ).await.unwrap().unwrap();
        assert_eq!(result.invested_amount, dec!(1010));
        assert_eq!(result.inventory, dec!(5.05));
        assert_eq!(result.realized, dec!(101));
        assert_eq!(result.unit_price, dec!(100));
    }
    #[test]
    fn position_threshold_treats_dust_as_closed() {
        assert!(is_position_size_over_threshold(dec!(1)));
        assert!(!is_position_size_over_threshold(dec!(0)));
        assert!(!is_position_size_over_threshold(dec!(0.00000000000001)));
        assert!(is_position_size_over_threshold(dec!(0.00000000000002)));
    }

}
