use anyhow::{Context, Result};
use chrono::Datelike;
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use log::{debug, info, trace};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use tabled::Tabled;
use typeshare::typeshare;

use crate::database::queries::stock_split::get_stock_splits;
use crate::database::queries::tax_optimization::get_tax_optimizations_by_date_range;
use crate::{
    database::queries::{
        composite::get_active_years, fund_report::get_oekb_fund_report_by_id,
        fx_rate::get_exchange_rate,
    },
    services::market_data::fx_rates::convert_amount,
    services::shared::constants::OUT_DIR,
};

use super::dtt::{self, determine_source_country, treaty_rate, DttIncomeType};
use super::instruments::stock_splits::{
    get_split_adjusted_price_per_unit, get_split_adjusted_units, StockSplit,
};
use super::{
    events::{get_events, EventType, PortfolioEvent, TradeDirection},
    files::export_json,
};

#[typeshare]
#[derive(Debug, Serialize, Tabled)]
pub struct AnnualTaxableAmounts {
    cash_interest: Decimal,
    share_lending_interest: Decimal,
    capital_gains: Decimal,
    capital_losses: Decimal,
    net_capital_gains: Decimal,
    dividends: Decimal,
    dividend_equivalents: Decimal,
    fx_appreciation: Decimal,
    withheld_tax_capital_gains: Decimal,
    withheld_tax_dividends: Decimal,
    withheld_tax_interest: Decimal,
    tax_optimization_adjustment: Decimal,
}

impl AnnualTaxableAmounts {
    fn round_all(&mut self, dp: u32) {
        trace!(target: "tax_report", "Rounding AnnualTaxableAmounts to {} decimal places", dp);
        let fields = [
            &mut self.cash_interest,
            &mut self.share_lending_interest,
            &mut self.capital_gains,
            &mut self.net_capital_gains,
            &mut self.dividends,
            &mut self.fx_appreciation,
            &mut self.dividend_equivalents,
            &mut self.capital_losses,
            &mut self.withheld_tax_capital_gains,
            &mut self.withheld_tax_dividends,
            &mut self.withheld_tax_interest,
            &mut self.tax_optimization_adjustment,
        ];
        for field in fields {
            *field = field.round_dp(dp);
        }
    }
}

#[typeshare]
#[derive(Debug, Serialize)]
pub struct TaxationReport {
    pub created_at: DateTime<Utc>,
    pub from_date: Option<DateTime<Utc>>,
    pub until_date: Option<DateTime<Utc>>,
    pub taxable_amounts: BTreeMap<i32, AnnualTaxableAmounts>,
    pub securities_wacs: BTreeMap<String, SecWac>,
    pub currency_wacs: BTreeMap<String, FxWac>,
}

#[typeshare]
#[derive(Debug, Clone, Tabled, Serialize)]
pub struct FxWac {
    pub units: Decimal,
    pub avg_rate: Decimal,
}

impl FxWac {
    fn round_all(&mut self) {
        trace!(target: "tax_report", "Rounding FX WAC values");

        self.units = self.units.round_dp(4);
        self.avg_rate = self.avg_rate.round_dp(2);
    }

    fn update(&mut self, new_units: Decimal, new_rate: Decimal) {
        debug!(target: "tax_report", "Updating FX WAC with {} units at rate {}", new_units, new_rate);

        let total_units = self.units + new_units;
        self.avg_rate = (self.units * self.avg_rate + new_units * new_rate) / total_units;
        self.units = total_units;
    }
}

#[typeshare]
#[derive(Debug, Clone, Tabled, Serialize)]
pub struct SecWac {
    pub units: Decimal,
    pub average_cost: Decimal,
    pub weighted_avg_fx_rate: Decimal,
    pub name: String,
}

impl SecWac {
    fn round_all(&mut self) {
        trace!(target: "tax_report", "Rounding SecWAC values");

        self.units = self.units.round_dp(4);
        self.average_cost = self.average_cost.round_dp(2);
        self.weighted_avg_fx_rate = self.weighted_avg_fx_rate.round_dp(2);
    }

    fn update(&mut self, event: &PortfolioEvent) -> Result<()> {
        debug!(target: "tax_report", "Updating security WAC for event: {:?}", event);

        let new_units = event.units;
        let new_cost = event.price_unit;
        let fx_rate = event
            .applied_fx_rate
            .context("Missing FX rate for security trade")?;

        let total_cost = self.units * self.average_cost + new_units * new_cost;

        if total_cost != dec!(0) {
            self.weighted_avg_fx_rate =
                (self.weighted_avg_fx_rate * self.units * self.average_cost
                    + new_units * new_cost * fx_rate)
                    / total_cost;
        } else {
            self.weighted_avg_fx_rate = dec!(0);
        }

        self.average_cost = total_cost / (self.units + new_units);
        self.units += new_units;

        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct TaxRates {
    pub interest: Decimal,
    pub capital_gains: Decimal,
    pub dividends: Decimal,
}

struct ProcessingContext<'a> {
    taxable_amounts: &'a mut BTreeMap<i32, AnnualTaxableAmounts>,
    currency_wacs: &'a mut BTreeMap<String, FxWac>,
    securities_wacs: &'a mut BTreeMap<String, SecWac>,
    tax_rates: &'a TaxRates,
    year: i32,
    stock_split_information: &'a mut [StockSplit],
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
}

impl ProcessingContext<'_> {
    fn get_year_entry(&mut self) -> &mut AnnualTaxableAmounts {
        self.taxable_amounts
            .entry(self.year)
            .or_insert_with(|| AnnualTaxableAmounts {
                cash_interest: dec!(0.0),
                share_lending_interest: dec!(0.0),
                capital_gains: dec!(0.0),
                net_capital_gains: dec!(0.0),
                dividends: dec!(0.0),
                fx_appreciation: dec!(0.0),
                dividend_equivalents: dec!(0.0),
                capital_losses: dec!(0.0),
                withheld_tax_capital_gains: dec!(0.0),
                withheld_tax_dividends: dec!(0.0),
                withheld_tax_interest: dec!(0.0),
                tax_optimization_adjustment: dec!(0.0),
            })
    }

    fn should_count_taxable(&self, event_date: DateTime<Utc>) -> bool {
        if let Some(from) = self.from_date {
            if event_date < from {
                return false;
            }
        }
        if let Some(until) = self.until_date {
            if event_date > until {
                return false;
            }
        }
        true
    }
}

async fn process_event(event: PortfolioEvent, ctx: &mut ProcessingContext<'_>) -> Result<()> {
    info!(target: "tax_report", "Processing event: {:?} ({:?}) on {:?}", event.identifier.clone().unwrap_or("No identifier".to_string()), event.event_type, event.date);

    match event.event_type {
        EventType::CashInterest | EventType::ShareInterest | EventType::Dividend => {
            process_interest_or_dividend(event, ctx).await
        }
        EventType::Trade => process_trade(event, ctx).await,
        EventType::FxConversion => process_fx_conversion(event, ctx).await,
        EventType::DividendAequivalent => process_dividend_aequivalent(event, ctx).await,
    }
}

async fn process_interest_or_dividend(
    event: PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
) -> Result<()> {
    debug!(target: "tax_report",
        "Processing {:?} event for {} {}",
        event.event_type,
        event.units,
        event.currency
    );

    let currency = &event.currency;

    // Use applied_fx_rate if available, otherwise fetch from database
    let fx_rate = match event.applied_fx_rate {
        Some(rate) => rate,
        None => {
            if currency == "EUR" {
                dec!(1.0)
            } else {
                // Fetch FX rate from database
                let naive_date = event.date.date_naive();
                get_exchange_rate(currency, "EUR", &naive_date)
                    .await
                    .unwrap_or(dec!(1.0))
            }
        }
    };

    if currency != "EUR" {
        ctx.currency_wacs
            .entry(currency.clone())
            .and_modify(|wac| wac.update(event.units, fx_rate))
            .or_insert(FxWac {
                units: event.units,
                avg_rate: fx_rate,
            });
    }

    let (taxed_amount, withheld_tax) = calculate_taxable_values(&event, ctx, fx_rate)?;
    let tax_type = match event.event_type {
        EventType::CashInterest => "interest",
        EventType::ShareInterest => "capital_gains",
        EventType::Dividend => "dividends",
        _ => unreachable!(),
    };

    if ctx.should_count_taxable(event.date) {
        apply_taxation(ctx, tax_type, taxed_amount, withheld_tax)
    } else {
        Ok(())
    }
}

fn calculate_taxable_values(
    event: &PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
    fx_rate: Decimal,
) -> Result<(Decimal, Decimal)> {
    let taxable_remainder = event.units * event.price_unit;

    let withheld_tax_percent = event.withholding_tax_percent.unwrap_or(dec!(0.0));

    let (income_type, austrian_rate) = match event.event_type {
        EventType::CashInterest => (DttIncomeType::Interest, ctx.tax_rates.interest),
        EventType::ShareInterest => (DttIncomeType::CapitalGains, ctx.tax_rates.capital_gains),
        EventType::Dividend => (DttIncomeType::Dividends, ctx.tax_rates.dividends),
        _ => unreachable!(),
    };

    let source_country = determine_source_country(event.identifier.as_deref(), &event.broker);

    let cap = match source_country {
        Some(country) => treaty_rate(country, income_type).unwrap_or(austrian_rate),
        None => austrian_rate,
    };

    let remaining_withholding_tax_percent = withheld_tax_percent.min(cap);

    let (taxed_amount, withheld_tax) = if event.currency == "EUR" {
        (
            taxable_remainder,
            remaining_withholding_tax_percent * taxable_remainder,
        )
    } else {
        (
            taxable_remainder / fx_rate,
            (remaining_withholding_tax_percent * taxable_remainder) * fx_rate,
        )
    };

    Ok((taxed_amount, withheld_tax))
}

fn apply_taxation(
    ctx: &mut ProcessingContext<'_>,
    tax_type: &str,
    taxed_amount: Decimal,
    withheld_tax: Decimal,
) -> Result<()> {
    let year_entry = ctx.get_year_entry();

    match tax_type {
        "interest" => {
            year_entry.cash_interest += taxed_amount + withheld_tax;
            year_entry.withheld_tax_interest += withheld_tax;
        }
        "capital_gains" => {
            year_entry.share_lending_interest += taxed_amount + withheld_tax;
        }
        "dividends" => {
            year_entry.dividends += taxed_amount + withheld_tax;
            year_entry.withheld_tax_dividends += withheld_tax;
        }
        _ => return Err(anyhow::anyhow!("Invalid tax type")),
    }

    Ok(())
}

async fn process_trade(event: PortfolioEvent, ctx: &mut ProcessingContext<'_>) -> Result<()> {
    debug!(target: "tax_report", "Processing trade of {} units", event.units);

    let direction = event.clone().direction.context("Missing trade direction")?;

    match direction {
        TradeDirection::Buy => process_buy(event, ctx).await,
        TradeDirection::Sell => process_sell(event, ctx).await,
    }
}

async fn process_buy(event: PortfolioEvent, ctx: &mut ProcessingContext<'_>) -> Result<()> {
    info!(target: "tax_report", "Processing BUY transaction for {:?}", event.identifier);

    ctx.securities_wacs
        .entry(
            event
                .identifier
                .clone()
                .context("Missing security identifier")?,
        )
        .and_modify(|sec_wac| {
            sec_wac
                .update(&event)
                .expect("Failed to update security WAC")
        })
        .or_insert({
            let mut sec_wac = SecWac {
                units: dec!(0.0),
                average_cost: dec!(0.0),
                weighted_avg_fx_rate: dec!(0.0),
                name: event
                    .name
                    .clone()
                    .unwrap_or(event.identifier.clone().unwrap()),
            };
            sec_wac.update(&event)?;
            sec_wac
        });

    if event.currency != "EUR" {
        process_fx_buy(event, ctx).await?;
    }

    Ok(())
}

async fn process_sell(event: PortfolioEvent, ctx: &mut ProcessingContext<'_>) -> Result<()> {
    info!(target: "tax_report", "Processing SELL transaction for {:?}", event.identifier);

    let identifier = event
        .identifier
        .clone()
        .context("Missing security identifier")?;
    let units = event.units;

    if let Some(sec_wac) = ctx.securities_wacs.get_mut(&identifier) {
        sec_wac.units = get_split_adjusted_units(
            &identifier,
            sec_wac.units,
            event.date,
            ctx.stock_split_information,
        );
        sec_wac.units -= units;
        sec_wac.average_cost = get_split_adjusted_price_per_unit(
            &identifier,
            sec_wac.average_cost,
            event.date,
            ctx.stock_split_information,
        )
    }

    if ctx.should_count_taxable(event.date) {
        if let Some(wht_percent) = event.withholding_tax_percent {
            let source_country = determine_source_country(Some(&identifier), &event.broker);
            let cap = match source_country {
                Some(country) => treaty_rate(country, DttIncomeType::CapitalGains)
                    .unwrap_or(ctx.tax_rates.capital_gains),
                None => ctx.tax_rates.capital_gains,
            };
            let wht_percent_to_consider = wht_percent.min(cap);
            let wht_currency_agnostic = wht_percent_to_consider * (event.price_unit * event.units);

            let withheld_tax = if event.currency == "EUR" {
                wht_currency_agnostic
            } else {
                wht_currency_agnostic * event.applied_fx_rate.unwrap()
            };
            ctx.get_year_entry().withheld_tax_capital_gains += withheld_tax;
        }

        if event.currency == "EUR" {
            process_eur_sell(event, ctx, &identifier)?;
        } else {
            process_fx_sell(event, ctx, &identifier).await?;
        }
    }

    Ok(())
}

fn process_eur_sell(
    event: PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
    identifier: &str,
) -> Result<()> {
    let sec_wac = ctx
        .securities_wacs
        .get(identifier)
        .context("Security WAC not found for sell transaction")?;

    let taxable_amount = (event.price_unit - sec_wac.average_cost) * event.units;
    let year_entry = ctx.get_year_entry();

    if taxable_amount > dec!(0.0) {
        year_entry.capital_gains += taxable_amount;
    } else {
        year_entry.capital_losses -= taxable_amount;
    }

    Ok(())
}

async fn process_fx_sell(
    event: PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
    identifier: &str,
) -> Result<()> {
    let sec_wac = ctx
        .securities_wacs
        .get(identifier)
        .context("Security WAC not found for FX sell")?;

    let gain_foreign = (event.price_unit - sec_wac.average_cost) * event.units;
    let eur_rate =
        convert_amount(dec!(1.0), &event.date.date_naive(), "EUR", &event.currency).await?;
    let gain_eur = gain_foreign / eur_rate;

    let fx_wac = ctx
        .currency_wacs
        .entry(event.currency.clone())
        .or_insert(FxWac {
            units: dec!(0.0),
            avg_rate: dec!(0.0),
        });

    let fx_rate_for_buy = if fx_wac.units > event.units * event.price_unit {
        fx_wac.avg_rate
    } else {
        sec_wac.weighted_avg_fx_rate
    };

    let original_eur_cost = (sec_wac.average_cost / fx_rate_for_buy) * event.units;
    let eur_sell = (event.price_unit / eur_rate) * event.units;
    let total_taxable = eur_sell - original_eur_cost;
    let fx_portion = total_taxable - gain_eur;

    let year_entry = ctx.get_year_entry();
    if gain_eur > dec!(0.0) {
        year_entry.capital_gains += gain_eur;
    } else {
        year_entry.capital_losses -= gain_eur;
    }
    year_entry.fx_appreciation += fx_portion;

    Ok(())
}

async fn process_fx_conversion(
    event: PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
) -> Result<()> {
    let direction = event
        .direction
        .as_ref()
        .context("Missing FX conversion direction")?;

    match direction {
        TradeDirection::Buy => process_fx_buy(event, ctx).await,
        TradeDirection::Sell => process_fx_sell_conversion(event, ctx).await,
    }
}

async fn process_fx_buy(event: PortfolioEvent, ctx: &mut ProcessingContext<'_>) -> Result<()> {
    let identifier = event.identifier.context("Missing FX identifier")?;

    let currency = if event.event_type == EventType::Trade {
        event.currency.clone()
    } else {
        identifier[identifier.len() - 3..].to_string()
    };

    if currency == "EUR" {
        return Ok(());
    }

    let fx_rate = event.applied_fx_rate.context("Missing FX rate")?;

    ctx.currency_wacs
        .entry(currency)
        .and_modify(|wac| wac.update(event.units, fx_rate))
        .or_insert(FxWac {
            units: event.units,
            avg_rate: fx_rate,
        });

    Ok(())
}

async fn process_fx_sell_conversion(
    event: PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
) -> Result<()> {
    let identifier = event.identifier.context("Missing FX identifier")?;
    let origin_currency = if event.event_type == EventType::Trade {
        event.currency.clone()
    } else {
        identifier[..3].to_string()
    };

    let eur_rate =
        convert_amount(dec!(1.0), &event.date.date_naive(), "EUR", &origin_currency).await?;

    let taxed_amount = {
        let fx_wac = ctx
            .currency_wacs
            .get_mut(&origin_currency)
            .context("Currency WAC not found for conversion")?;

        let fx_delta = fx_wac.avg_rate - eur_rate;
        let taxed_amount = ((fx_delta / eur_rate) * event.units) / eur_rate;

        fx_wac.units -= event.units;
        if fx_wac.units < dec!(0.0) {
            fx_wac.units = dec!(0.0);
        }

        taxed_amount
    };

    if ctx.should_count_taxable(event.date) {
        ctx.get_year_entry().fx_appreciation += taxed_amount;
    }

    Ok(())
}

async fn process_dividend_aequivalent(
    event: PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
) -> Result<()> {
    let report_id = event
        .identifier
        .clone()
        .context("Missing fund report ID")?
        .parse::<i32>()?;
    let full_report = get_oekb_fund_report_by_id(report_id).await?;

    let units_held = {
        let wacs = ctx
            .securities_wacs
            .entry(full_report.isin.clone())
            .or_insert(SecWac {
                units: dec!(0.0),
                average_cost: dec!(0.0),
                weighted_avg_fx_rate: dec!(1.0),
                name: full_report.isin.clone(),
            });
        wacs.units
    };

    let cost_adjustment = convert_amount(
        full_report.wac_adjustment,
        &full_report.date.date_naive(),
        &full_report.currency,
        "EUR",
    )
    .await?;

    if let Some(sec_wac) = ctx.securities_wacs.get_mut(&full_report.isin) {
        sec_wac.average_cost += cost_adjustment;
    }

    if ctx.should_count_taxable(event.date) {
        let taxed_amount =
            (full_report.dividend_aequivalent + full_report.intermittent_dividends) * units_held;

        let taxed_eur = convert_amount(
            taxed_amount,
            &full_report.date.date_naive(),
            &full_report.currency,
            "EUR",
        )
        .await?;

        let withheld_tax = full_report.withheld_dividend * units_held;
        let withheld_eur = convert_amount(
            withheld_tax,
            &full_report.date.date_naive(),
            &full_report.currency,
            "EUR",
        )
        .await?;

        let year_entry = ctx.get_year_entry();
        year_entry.dividend_equivalents += taxed_eur;
        year_entry.withheld_tax_dividends += withheld_eur;
    }

    Ok(())
}

pub async fn get_capital_gains_tax_report(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<TaxationReport> {
    info!(target: "tax_report", "Starting capital gains tax report generation (from={:?}, until={:?})", from_date, until_date);

    let mut stock_split_information = get_stock_splits().await?;

    debug!(target: "tax_report", "Loaded {} stock splits", stock_split_information.len());

    let tax_rates = TaxRates {
        interest: dec!(0.25),
        capital_gains: dec!(0.275),
        dividends: dec!(0.275),
    };

    info!(target: "tax_report", "Using tax rates: Interest {}%, Capital Gains {}%, Dividends {}%",
        tax_rates.interest * dec!(100),
        tax_rates.capital_gains * dec!(100),
        tax_rates.dividends * dec!(100)
    );

    let tax_relevant_years = get_active_years().await?;
    let mut taxable_amounts = BTreeMap::new();
    let mut currency_wacs = BTreeMap::new();
    let mut securities_wacs = BTreeMap::new();

    for year in tax_relevant_years {
        let mut ctx = ProcessingContext {
            taxable_amounts: &mut taxable_amounts,
            currency_wacs: &mut currency_wacs,
            securities_wacs: &mut securities_wacs,
            tax_rates: &tax_rates,
            year,
            stock_split_information: &mut stock_split_information,
            from_date,
            until_date,
        };

        let start_date = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).unwrap();
        let end_date = Utc.with_ymd_and_hms(year, 12, 31, 23, 59, 59).unwrap();
        let events = get_events(start_date, end_date)
            .await
            .with_context(|| format!("Failed to load events for year {}", year))?;
        for event in events {
            let event_desc = format!(
                "Failed to process event {:?} ({:?}) on {}",
                event.identifier.as_deref().unwrap_or("No identifier"),
                event.event_type,
                event.date
            );
            process_event(event, &mut ctx)
                .await
                .with_context(|| event_desc)?;
        }

        // Apply tax optimizations for this year
        let tax_optimizations = get_tax_optimizations_by_date_range(start_date, end_date).await?;
        for opt in tax_optimizations {
            if let Some(from) = from_date {
                if opt.date < from {
                    continue;
                }
            }
            if let Some(until) = until_date {
                if opt.date > until {
                    continue;
                }
            }

            // Skip zero-amount tax optimizations — they are no-ops but create log noise
            if opt.amount == dec!(0) {
                continue;
            }

            let year_entry = ctx.get_year_entry();
            // Tax optimization: negative amount = additional tax paid (increase withheld)
            // positive amount = tax refund (decrease withheld)
            match opt.tax_type.as_str() {
                "CapitalGains" => {
                    year_entry.withheld_tax_capital_gains -= opt.amount;
                    year_entry.tax_optimization_adjustment -= opt.amount;
                }
                "Dividend" => {
                    year_entry.withheld_tax_dividends -= opt.amount;
                    year_entry.tax_optimization_adjustment -= opt.amount;
                }
                "Interest" => {
                    year_entry.withheld_tax_interest -= opt.amount;
                    year_entry.tax_optimization_adjustment -= opt.amount;
                }
                _ => {
                    // Default to capital gains if type is unknown
                    year_entry.withheld_tax_capital_gains -= opt.amount;
                    year_entry.tax_optimization_adjustment -= opt.amount;
                }
            }
            info!(target: "tax_report",
                "Applied tax optimization for {}: {} EUR (type: {})",
                year, opt.amount, opt.tax_type
            );
        }
    }

    post_process(
        &mut taxable_amounts,
        &mut currency_wacs,
        &mut securities_wacs,
    );

    let report = TaxationReport {
        created_at: Utc::now(),
        from_date,
        until_date,
        taxable_amounts,
        securities_wacs,
        currency_wacs,
    };

    if from_date.is_none() && until_date.is_none() {
        info!(target: "tax_report", "Exporting taxation report to JSON");
        export_json(&report, "taxation")?;
    }
    info!(target: "tax_report", "Tax report generated successfully");

    Ok(report)
}

fn post_process(
    taxable_amounts: &mut BTreeMap<i32, AnnualTaxableAmounts>,
    currency_wacs: &mut BTreeMap<String, FxWac>,
    securities_wacs: &mut BTreeMap<String, SecWac>,
) {
    for amounts in taxable_amounts.values_mut() {
        amounts.net_capital_gains = (amounts.capital_gains - amounts.capital_losses).max(dec!(0.0));
        amounts.round_all(2);
    }

    currency_wacs.retain(|_, wac| wac.units > dec!(0));
    securities_wacs.retain(|_, sec_wac| sec_wac.units > dec!(0));

    for wac in currency_wacs.values_mut() {
        wac.round_all();
    }
    for sec_wac in securities_wacs.values_mut() {
        sec_wac.round_all();
    }
    info!(target: "tax_report", "Post-processing report data");
}

struct ImpactContext {
    currency_wacs: BTreeMap<String, FxWac>,
    securities_wacs: BTreeMap<String, SecWac>,
    tax_rates: TaxRates,
    stock_split_information: Vec<StockSplit>,
}

impl ImpactContext {
    fn new(tax_rates: TaxRates, stock_split_information: Vec<StockSplit>) -> Self {
        Self {
            currency_wacs: BTreeMap::new(),
            securities_wacs: BTreeMap::new(),
            tax_rates,
            stock_split_information,
        }
    }
}

pub async fn get_transaction_tax_impacts(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<Vec<TransactionTaxImpact>> {
    info!(target: "tax_report", "Starting transaction tax impact generation");

    let stock_split_information = get_stock_splits().await?;

    let tax_rates = TaxRates {
        interest: dec!(0.25),
        capital_gains: dec!(0.275),
        dividends: dec!(0.275),
    };

    let active_years = get_active_years().await?;
    let first_year = active_years
        .first()
        .copied()
        .unwrap_or_else(|| Utc::now().year());
    let last_year = active_years
        .last()
        .copied()
        .unwrap_or_else(|| Utc::now().year());

    let event_start = Utc.with_ymd_and_hms(first_year, 1, 1, 0, 0, 0).unwrap();
    let event_end = Utc.with_ymd_and_hms(last_year, 12, 31, 23, 59, 59).unwrap();

    let all_events = get_events(event_start, event_end).await?;

    let mut ctx = ImpactContext::new(tax_rates, stock_split_information);
    let mut impacts = Vec::new();

    for event in all_events {
        let impact = compute_event_impact(&event, &mut ctx).await;
        match impact {
            Ok(impact) => {
                // Filter to requested date range
                let in_range = match (from_date, until_date) {
                    (Some(from), Some(until)) => event.date >= from && event.date <= until,
                    (Some(from), None) => event.date >= from,
                    (None, Some(until)) => event.date <= until,
                    (None, None) => true,
                };
                if in_range {
                    impacts.push(impact);
                }
            }
            Err(e) => {
                log::warn!(target: "tax_report", "Failed to compute impact for event {:?} on {}: {}", event.identifier, event.date, e);
                // Push a fallback impact so the event still appears
                let in_range = match (from_date, until_date) {
                    (Some(from), Some(until)) => event.date >= from && event.date <= until,
                    (Some(from), None) => event.date >= from,
                    (None, Some(until)) => event.date <= until,
                    (None, None) => true,
                };
                if in_range {
                    impacts.push(TransactionTaxImpact {
                        date: event.date,
                        event_type: event.event_type.clone(),
                        identifier: event.identifier.clone(),
                        name: event.name.clone(),
                        direction: event.direction.clone(),
                        currency: event.currency.clone(),
                        units: event.units,
                        price_unit: event.price_unit,
                        total: event.total,
                        broker: event.broker.clone(),
                        impact_type: "Error".to_string(),
                        taxable_amount: dec!(0.0),
                        withheld_tax: dec!(0.0),
                        tax_liability: dec!(0.0),
                        tax_rate_percent: dec!(0.0),
                        source_country: None,
                        dtt_rate_percent: None,
                        notes: format!("Error computing impact: {}", e),
                        is_tax_relevant: false,
                    });
                }
            }
        }
    }

    info!(target: "tax_report", "Generated {} transaction tax impacts", impacts.len());
    Ok(impacts)
}

async fn compute_event_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    match event.event_type {
        EventType::CashInterest | EventType::ShareInterest | EventType::Dividend => {
            compute_interest_or_dividend_impact(event, ctx).await
        }
        EventType::Trade => compute_trade_impact(event, ctx).await,
        EventType::FxConversion => compute_fx_conversion_impact(event, ctx).await,
        EventType::DividendAequivalent => compute_dividend_aequivalent_impact(event, ctx).await,
    }
}

async fn compute_interest_or_dividend_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    let currency = &event.currency;

    let fx_rate = match event.applied_fx_rate {
        Some(rate) => rate,
        None => {
            if currency == "EUR" {
                dec!(1.0)
            } else {
                let naive_date = event.date.date_naive();
                get_exchange_rate(currency, "EUR", &naive_date)
                    .await
                    .unwrap_or(dec!(1.0))
            }
        }
    };

    if currency != "EUR" {
        ctx.currency_wacs
            .entry(currency.clone())
            .and_modify(|wac| wac.update(event.units, fx_rate))
            .or_insert(FxWac {
                units: event.units,
                avg_rate: fx_rate,
            });
    }

    let taxable_remainder = event.units * event.price_unit;
    let raw_wht_percent = event.withholding_tax_percent.unwrap_or(dec!(0.0));

    let (income_type, austrian_rate) = match event.event_type {
        EventType::CashInterest => (DttIncomeType::Interest, ctx.tax_rates.interest),
        EventType::ShareInterest => (DttIncomeType::CapitalGains, ctx.tax_rates.capital_gains),
        EventType::Dividend => (DttIncomeType::Dividends, ctx.tax_rates.dividends),
        _ => unreachable!(),
    };

    let source_country = determine_source_country(event.identifier.as_deref(), &event.broker);

    let (cap, dtt_rate_percent) = match source_country {
        Some(country) => {
            let rate = treaty_rate(country, income_type).unwrap_or(austrian_rate);
            (rate, Some(rate * dec!(100)))
        }
        None => (austrian_rate, None),
    };

    let capped_wht_percent = match event.event_type {
        EventType::ShareInterest => dec!(0.0),
        _ => raw_wht_percent.min(cap),
    };

    let (taxed_amount, withheld_tax) = if event.currency == "EUR" {
        (taxable_remainder, capped_wht_percent * taxable_remainder)
    } else {
        (
            taxable_remainder / fx_rate,
            (capped_wht_percent * taxable_remainder) * fx_rate,
        )
    };

    let (impact_type, tax_rate, base_notes) = match event.event_type {
        EventType::CashInterest => (
            "Interest".to_string(),
            ctx.tax_rates.interest * dec!(100),
            "Interest income".to_string(),
        ),
        EventType::ShareInterest => (
            "Share Lending Interest".to_string(),
            ctx.tax_rates.capital_gains * dec!(100),
            "Share lending income".to_string(),
        ),
        EventType::Dividend => (
            "Dividend".to_string(),
            ctx.tax_rates.dividends * dec!(100),
            "Dividend income".to_string(),
        ),
        _ => unreachable!(),
    };

    let gross_tax = taxed_amount.max(dec!(0.0)) * (tax_rate / dec!(100));
    let tax_liability = (gross_tax - withheld_tax).max(dec!(0.0));

    let dtt_note = match (&source_country, dtt_rate_percent) {
        (Some(country), Some(rate)) => format!("DTT {}: {}%", country, rate),
        (None, _) => format!("No DTT (fallback cap: {}%)", cap * dec!(100)),
        _ => format!("No DTT"),
    };

    let notes = format!(
        "{}. Gross tax: {}, WHT credit: {}, Tax to pay: {}. {}",
        base_notes,
        format_currency_value(gross_tax),
        format_currency_value(withheld_tax),
        format_currency_value(tax_liability),
        dtt_note
    );

    Ok(TransactionTaxImpact {
        date: event.date,
        event_type: event.event_type.clone(),
        identifier: event.identifier.clone(),
        name: event.name.clone(),
        direction: event.direction.clone(),
        currency: event.currency.clone(),
        units: event.units,
        price_unit: event.price_unit,
        total: event.total,
        broker: event.broker.clone(),
        impact_type,
        taxable_amount: taxed_amount.round_dp(2),
        withheld_tax: withheld_tax.round_dp(2),
        tax_liability: tax_liability.round_dp(2),
        tax_rate_percent: tax_rate.round_dp(1),
        source_country: source_country.map(|c| c.to_string()),
        dtt_rate_percent: dtt_rate_percent.map(|r| r.round_dp(1)),
        notes,
        is_tax_relevant: taxed_amount != dec!(0.0),
    })
}

async fn compute_trade_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    let direction = event.direction.clone().context("Missing trade direction")?;

    match direction {
        TradeDirection::Buy => compute_buy_impact(event, ctx).await,
        TradeDirection::Sell => compute_sell_impact(event, ctx).await,
    }
}

async fn compute_buy_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    let identifier = event
        .identifier
        .clone()
        .context("Missing security identifier")?;

    ctx.securities_wacs
        .entry(identifier.clone())
        .and_modify(|sec_wac| {
            sec_wac
                .update(event)
                .expect("Failed to update security WAC")
        })
        .or_insert({
            let mut sec_wac = SecWac {
                units: dec!(0.0),
                average_cost: dec!(0.0),
                weighted_avg_fx_rate: dec!(0.0),
                name: event.name.clone().unwrap_or(identifier.clone()),
            };
            sec_wac.update(event).unwrap();
            sec_wac
        });

    if event.currency != "EUR" {
        let currency = event.currency.clone();

        if currency != "EUR" {
            let fx_rate = event.applied_fx_rate.context("Missing FX rate")?;
            ctx.currency_wacs
                .entry(currency)
                .and_modify(|wac| wac.update(event.units, fx_rate))
                .or_insert(FxWac {
                    units: event.units,
                    avg_rate: fx_rate,
                });
        }
    }

    let wac = ctx
        .securities_wacs
        .get(&identifier)
        .map(|w| w.average_cost)
        .unwrap_or(dec!(0.0));

    Ok(TransactionTaxImpact {
        date: event.date,
        event_type: event.event_type.clone(),
        identifier: event.identifier.clone(),
        name: event.name.clone(),
        direction: event.direction.clone(),
        currency: event.currency.clone(),
        units: event.units,
        price_unit: event.price_unit,
        total: event.total,
        broker: event.broker.clone(),
        impact_type: "Buy".to_string(),
        taxable_amount: dec!(0.0),
        withheld_tax: dec!(0.0),
        tax_liability: dec!(0.0),
        tax_rate_percent: dec!(0.0),
        source_country: None,
        dtt_rate_percent: None,
        notes: format!(
            "Added {} units at {}. New WAC: {}",
            event.units,
            format_currency_value(event.price_unit),
            format_currency_value(wac)
        ),
        is_tax_relevant: false,
    })
}

async fn compute_sell_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    let identifier = event
        .identifier
        .clone()
        .context("Missing security identifier")?;
    let units = event.units;

    let sec_wac = ctx.securities_wacs.get_mut(&identifier).cloned();

    if let Some(ref mut sec_wac) = ctx.securities_wacs.get_mut(&identifier) {
        sec_wac.units = get_split_adjusted_units(
            &identifier,
            sec_wac.units,
            event.date,
            &mut ctx.stock_split_information,
        );
        sec_wac.units -= units;
        sec_wac.average_cost = get_split_adjusted_price_per_unit(
            &identifier,
            sec_wac.average_cost,
            event.date,
            &mut ctx.stock_split_information,
        );
    }

    let sec_wac_ref = sec_wac.as_ref();
    let wac_cost = sec_wac_ref.map(|w| w.average_cost).unwrap_or(dec!(0.0));

    let (taxable_amount, gross_tax, impact_type, notes) = if event.currency == "EUR" {
        let gain = (event.price_unit - wac_cost) * units;
        let tax = gain.max(dec!(0.0)) * ctx.tax_rates.capital_gains;
        let impact_type = if gain > dec!(0.0) {
            "Capital Gain"
        } else if gain < dec!(0.0) {
            "Capital Loss"
        } else {
            "Sell"
        };
        let notes = format!(
            "Sold {} units at {} (WAC: {}). Gain/Loss: {}",
            units,
            format_currency_value(event.price_unit),
            format_currency_value(wac_cost),
            format_currency_value(gain)
        );
        (gain, tax, impact_type.to_string(), notes)
    } else {
        let gain_foreign = (event.price_unit - wac_cost) * units;
        let eur_rate =
            convert_amount(dec!(1.0), &event.date.date_naive(), "EUR", &event.currency).await?;
        let gain_eur = gain_foreign / eur_rate;

        let fx_wac = ctx.currency_wacs.get_mut(&event.currency).cloned();

        let fx_rate_for_buy = if let Some(ref fx) = fx_wac {
            if fx.units > event.units * event.price_unit {
                fx.avg_rate
            } else {
                sec_wac_ref
                    .map(|w| w.weighted_avg_fx_rate)
                    .unwrap_or(dec!(0.0))
            }
        } else {
            sec_wac_ref
                .map(|w| w.weighted_avg_fx_rate)
                .unwrap_or(dec!(0.0))
        };

        let original_eur_cost = (wac_cost / fx_rate_for_buy) * units;
        let eur_sell = (event.price_unit / eur_rate) * units;
        let total_taxable = eur_sell - original_eur_cost;
        let fx_portion = total_taxable - gain_eur;

        let tax = total_taxable.max(dec!(0.0)) * ctx.tax_rates.capital_gains;

        let impact_type = if total_taxable > dec!(0.0) {
            "Capital Gain"
        } else if total_taxable < dec!(0.0) {
            "Capital Loss"
        } else {
            "Sell"
        };

        let notes = format!(
            "Sold {} units at {} {} (WAC: {} {}). Security gain: {} EUR, FX gain/loss: {} EUR. Total: {} EUR",
            units,
            format_currency_value(event.price_unit),
            event.currency,
            format_currency_value(wac_cost),
            event.currency,
            format_currency_value(gain_eur),
            format_currency_value(fx_portion),
            format_currency_value(total_taxable)
        );

        if let Some(ref mut fx) = ctx.currency_wacs.get_mut(&event.currency) {
            fx.units -= event.units * event.price_unit;
            if fx.units < dec!(0.0) {
                fx.units = dec!(0.0);
            }
        }

        (total_taxable, tax, impact_type.to_string(), notes)
    };

    let source_country = determine_source_country(event.identifier.as_deref(), &event.broker);

    let (cap, dtt_rate_percent) = match source_country {
        Some(country) => {
            let rate = treaty_rate(country, DttIncomeType::CapitalGains)
                .unwrap_or(ctx.tax_rates.capital_gains);
            (rate, Some(rate * dec!(100)))
        }
        None => (ctx.tax_rates.capital_gains, None),
    };

    let withheld_tax = if let Some(wht_percent) = event.withholding_tax_percent {
        let wht_amount = wht_percent.min(cap) * (event.price_unit * units);
        if event.currency == "EUR" {
            wht_amount
        } else {
            wht_amount * event.applied_fx_rate.unwrap_or(dec!(1.0))
        }
    } else {
        dec!(0.0)
    };

    let tax_liability = (gross_tax - withheld_tax).max(dec!(0.0));

    let dtt_note = match (&source_country, dtt_rate_percent) {
        (Some(country), Some(rate)) => format!("DTT {}: {}%", country, rate),
        (None, _) => format!("No DTT (fallback cap: {}%)", cap * dec!(100)),
        _ => "No DTT".to_string(),
    };

    let notes = format!(
        "{}. Gross tax: {}, WHT credit: {}, Tax to pay: {}. {}",
        notes,
        format_currency_value(gross_tax),
        format_currency_value(withheld_tax),
        format_currency_value(tax_liability),
        dtt_note
    );

    Ok(TransactionTaxImpact {
        date: event.date,
        event_type: event.event_type.clone(),
        identifier: event.identifier.clone(),
        name: event.name.clone(),
        direction: event.direction.clone(),
        currency: event.currency.clone(),
        units: event.units,
        price_unit: event.price_unit,
        total: event.total,
        broker: event.broker.clone(),
        impact_type,
        taxable_amount: taxable_amount.round_dp(2),
        withheld_tax: withheld_tax.round_dp(2),
        tax_liability: tax_liability.round_dp(2),
        tax_rate_percent: (ctx.tax_rates.capital_gains * dec!(100)).round_dp(1),
        source_country: source_country.map(|c| c.to_string()),
        dtt_rate_percent: dtt_rate_percent.map(|r| r.round_dp(1)),
        notes,
        is_tax_relevant: taxable_amount != dec!(0.0),
    })
}

async fn compute_fx_conversion_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    let identifier = event.identifier.clone().context("Missing FX identifier")?;
    let direction = event
        .direction
        .clone()
        .context("Missing FX conversion direction")?;

    match direction {
        TradeDirection::Buy => {
            let currency = identifier[identifier.len() - 3..].to_string();
            let fx_rate = event.applied_fx_rate.unwrap_or(dec!(1.0));

            if currency != "EUR" {
                let rate = event.applied_fx_rate.context("Missing FX rate")?;
                ctx.currency_wacs
                    .entry(currency.clone())
                    .and_modify(|wac| wac.update(event.units, rate))
                    .or_insert(FxWac {
                        units: event.units,
                        avg_rate: rate,
                    });
            }

            Ok(TransactionTaxImpact {
                date: event.date,
                event_type: event.event_type.clone(),
                identifier: event.identifier.clone(),
                name: event.name.clone(),
                direction: event.direction.clone(),
                currency: event.currency.clone(),
                units: event.units,
                price_unit: event.price_unit,
                total: event.total,
                broker: event.broker.clone(),
                impact_type: "FX Buy".to_string(),
                taxable_amount: dec!(0.0),
                withheld_tax: dec!(0.0),
                tax_liability: dec!(0.0),
                tax_rate_percent: dec!(0.0),
                source_country: None,
                dtt_rate_percent: None,
                notes: format!(
                    "Bought {} {} at rate {}",
                    event.units,
                    currency,
                    format_currency_value(fx_rate)
                ),
                is_tax_relevant: false,
            })
        }
        TradeDirection::Sell => {
            let origin_currency = identifier[..3].to_string();
            let eur_rate =
                convert_amount(dec!(1.0), &event.date.date_naive(), "EUR", &origin_currency)
                    .await?;

            let fx_wac = ctx.currency_wacs.get_mut(&origin_currency).cloned();

            let taxed_amount =
                if let Some(ref mut fx_wac_mut) = ctx.currency_wacs.get_mut(&origin_currency) {
                    let fx_delta = fx_wac_mut.avg_rate - eur_rate;
                    let taxed = ((fx_delta / eur_rate) * event.units) / eur_rate;
                    fx_wac_mut.units -= event.units;
                    if fx_wac_mut.units < dec!(0.0) {
                        fx_wac_mut.units = dec!(0.0);
                    }
                    taxed
                } else {
                    dec!(0.0)
                };

            let tax_liability = taxed_amount.max(dec!(0.0)) * ctx.tax_rates.capital_gains;

            let impact_type = if taxed_amount > dec!(0.0) {
                "FX Appreciation"
            } else if taxed_amount < dec!(0.0) {
                "FX Depreciation"
            } else {
                "FX Sell"
            };

            let wac_rate = fx_wac.map(|w| w.avg_rate).unwrap_or(dec!(0.0));

            let notes = format!(
                "Sold {} {}. WAC rate: {}, current rate: {}. FX delta: {}",
                event.units,
                origin_currency,
                format_currency_value(wac_rate),
                format_currency_value(eur_rate),
                format_currency_value(taxed_amount)
            );

            Ok(TransactionTaxImpact {
                date: event.date,
                event_type: event.event_type.clone(),
                identifier: event.identifier.clone(),
                name: event.name.clone(),
                direction: event.direction.clone(),
                currency: event.currency.clone(),
                units: event.units,
                price_unit: event.price_unit,
                total: event.total,
                broker: event.broker.clone(),
                impact_type: impact_type.to_string(),
                taxable_amount: taxed_amount.round_dp(2),
                withheld_tax: dec!(0.0),
                tax_liability: tax_liability.round_dp(2),
                tax_rate_percent: (ctx.tax_rates.capital_gains * dec!(100)).round_dp(1),
                source_country: None,
                dtt_rate_percent: None,
                notes,
                is_tax_relevant: taxed_amount != dec!(0.0),
            })
        }
    }
}

async fn compute_dividend_aequivalent_impact(
    event: &PortfolioEvent,
    ctx: &mut ImpactContext,
) -> Result<TransactionTaxImpact> {
    let report_id = event
        .identifier
        .clone()
        .context("Missing fund report ID")?
        .parse::<i32>()?;
    let full_report = get_oekb_fund_report_by_id(report_id).await?;

    let units_held = {
        let wacs = ctx
            .securities_wacs
            .entry(full_report.isin.clone())
            .or_insert(SecWac {
                units: dec!(0.0),
                average_cost: dec!(0.0),
                weighted_avg_fx_rate: dec!(1.0),
                name: full_report.isin.clone(),
            });
        wacs.units
    };

    let cost_adjustment = convert_amount(
        full_report.wac_adjustment,
        &full_report.date.date_naive(),
        &full_report.currency,
        "EUR",
    )
    .await?;

    if let Some(sec_wac) = ctx.securities_wacs.get_mut(&full_report.isin) {
        sec_wac.average_cost += cost_adjustment;
    }

    let taxed_amount =
        (full_report.dividend_aequivalent + full_report.intermittent_dividends) * units_held;

    let taxed_eur = convert_amount(
        taxed_amount,
        &full_report.date.date_naive(),
        &full_report.currency,
        "EUR",
    )
    .await?;

    let tax_liability = taxed_eur.max(dec!(0.0)) * ctx.tax_rates.dividends;

    let notes = format!(
        "Fund report dividend equivalent for {} units of {}. Amount: {} {}",
        units_held,
        full_report.isin,
        format_currency_value(taxed_amount),
        full_report.currency
    );

    Ok(TransactionTaxImpact {
        date: event.date,
        event_type: event.event_type.clone(),
        identifier: Some(full_report.isin.clone()),
        name: event.name.clone(),
        direction: event.direction.clone(),
        currency: full_report.currency.clone(),
        units: units_held,
        price_unit: full_report.dividend_aequivalent + full_report.intermittent_dividends,
        total: event.total,
        broker: event.broker.clone(),
        impact_type: "Dividend Equivalent".to_string(),
        taxable_amount: taxed_eur.round_dp(2),
        withheld_tax: dec!(0.0),
        tax_liability: tax_liability.round_dp(2),
        tax_rate_percent: (ctx.tax_rates.dividends * dec!(100)).round_dp(1),
        source_country: dtt::isin_to_country(&full_report.isin).map(|c| c.to_string()),
        dtt_rate_percent: dtt::treaty_rate(
            dtt::isin_to_country(&full_report.isin).unwrap_or(""),
            DttIncomeType::Dividends,
        )
        .map(|r| (r * dec!(100)).round_dp(1)),
        notes,
        is_tax_relevant: taxed_eur != dec!(0.0),
    })
}

fn format_currency_value(value: Decimal) -> String {
    format!("{:.2}", value)
}

#[derive(Debug, Serialize)]
pub struct DetailedTaxationReport {
    pub report: TaxationReport,
    pub tax_rates: TaxRates,
    pub events_by_year: BTreeMap<i32, Vec<PortfolioEvent>>,
    pub metadata: TaxReportMetadata,
    pub transaction_impacts: Vec<TransactionTaxImpact>,
}

#[derive(Debug, Serialize)]
pub struct TaxReportMetadata {
    pub earliest_event_date: Option<DateTime<Utc>>,
    pub latest_event_date: Option<DateTime<Utc>>,
    pub total_events: usize,
    pub unique_securities: Vec<String>,
    pub unique_currencies: Vec<String>,
}

#[typeshare]
#[derive(Debug, Serialize)]
pub struct TransactionTaxImpact {
    pub date: DateTime<Utc>,
    pub event_type: EventType,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub direction: Option<TradeDirection>,
    pub currency: String,
    pub units: Decimal,
    pub price_unit: Decimal,
    pub total: Decimal,
    pub broker: String,
    pub impact_type: String,
    pub taxable_amount: Decimal,
    pub withheld_tax: Decimal,
    pub tax_liability: Decimal,
    pub tax_rate_percent: Decimal,
    pub source_country: Option<String>,
    pub dtt_rate_percent: Option<Decimal>,
    pub notes: String,
    pub is_tax_relevant: bool,
}

pub async fn get_detailed_capital_gains_tax_report(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<DetailedTaxationReport> {
    info!(target: "tax_report", "Starting detailed capital gains tax report generation");

    // 1. Generate the base tax report
    let report = get_capital_gains_tax_report(from_date, until_date).await?;

    // 2. Determine the full event date range (all active years)
    let active_years = get_active_years().await?;
    let first_year = active_years
        .first()
        .copied()
        .unwrap_or_else(|| Utc::now().year());
    let last_year = active_years
        .last()
        .copied()
        .unwrap_or_else(|| Utc::now().year());

    let event_start = Utc.with_ymd_and_hms(first_year, 1, 1, 0, 0, 0).unwrap();
    let event_end = Utc.with_ymd_and_hms(last_year, 12, 31, 23, 59, 59).unwrap();

    // 3. Fetch all events in the full range
    let all_events = get_events(event_start, event_end).await?;

    // 4. Group events by year
    let mut events_by_year: BTreeMap<i32, Vec<PortfolioEvent>> = BTreeMap::new();
    for event in all_events {
        let year = event.date.year();
        events_by_year.entry(year).or_default().push(event);
    }

    // 5. Collect metadata
    let mut unique_securities: HashSet<String> = HashSet::new();
    let mut unique_currencies: HashSet<String> = HashSet::new();
    let mut earliest_event_date: Option<DateTime<Utc>> = None;
    let mut latest_event_date: Option<DateTime<Utc>> = None;

    for events in events_by_year.values() {
        for event in events {
            if let Some(ref id) = event.identifier {
                if event.event_type == EventType::Trade
                    || event.event_type == EventType::DividendAequivalent
                {
                    unique_securities.insert(id.clone());
                }
            }
            unique_currencies.insert(event.currency.clone());

            if earliest_event_date.map_or(true, |d| event.date < d) {
                earliest_event_date = Some(event.date);
            }
            if latest_event_date.map_or(true, |d| event.date > d) {
                latest_event_date = Some(event.date);
            }
        }
    }

    let metadata = TaxReportMetadata {
        earliest_event_date,
        latest_event_date,
        total_events: events_by_year.values().map(|v| v.len()).sum(),
        unique_securities: unique_securities.into_iter().collect(),
        unique_currencies: unique_currencies.into_iter().collect(),
    };

    let tax_rates = TaxRates {
        interest: dec!(0.25),
        capital_gains: dec!(0.275),
        dividends: dec!(0.275),
    };

    let transaction_impacts = get_transaction_tax_impacts(from_date, until_date)
        .await
        .unwrap_or_default();

    let detailed_report = DetailedTaxationReport {
        report,
        tax_rates,
        events_by_year,
        metadata,
        transaction_impacts,
    };

    Ok(detailed_report)
}

pub async fn export_detailed_capital_gains_tax_report(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<()> {
    let detailed_report = get_detailed_capital_gains_tax_report(from_date, until_date).await?;

    info!(target: "tax_report", "Exporting detailed taxation report to JSON");
    export_json(&detailed_report, "taxation_detailed")?;
    info!(target: "tax_report", "Detailed tax report exported successfully to {}/taxation_detailed.json", OUT_DIR);

    Ok(())
}
