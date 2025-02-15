use anyhow::{Context, Result};
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use log::{debug, info, trace};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use std::collections::BTreeMap;
use tabled::Tabled;
use typeshare::typeshare;

use crate::database::queries::stock_split::get_stock_splits;
use crate::{
    database::queries::{composite::get_active_years, fund_report::get_oekb_fund_report_by_id},
    services::market_data::fx_rates::convert_amount,
};

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
    dividends: Decimal,
    dividend_equivalents: Decimal,
    fx_appreciation: Decimal,
    withheld_tax_capital_gains: Decimal,
    withheld_tax_dividends: Decimal,
    withheld_tax_interest: Decimal,
}

impl AnnualTaxableAmounts {
    fn round_all(&mut self, dp: u32) {
        trace!(target: "tax_report", "Rounding AnnualTaxableAmounts to {} decimal places", dp);
        let fields = [
            &mut self.cash_interest,
            &mut self.share_lending_interest,
            &mut self.capital_gains,
            &mut self.dividends,
            &mut self.fx_appreciation,
            &mut self.dividend_equivalents,
            &mut self.capital_losses,
            &mut self.withheld_tax_dividends,
            &mut self.withheld_tax_interest,
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
    pub taxable_amounts: BTreeMap<i32, AnnualTaxableAmounts>,
    pub securities_wacs: BTreeMap<String, SecWac>,
    pub currency_wacs: BTreeMap<String, FxWac>,
}

#[typeshare]
#[derive(Debug, Tabled, Serialize)]
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
#[derive(Debug, Tabled, Serialize)]
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
}

impl ProcessingContext<'_> {
    fn get_year_entry(&mut self) -> &mut AnnualTaxableAmounts {
        self.taxable_amounts
            .entry(self.year)
            .or_insert_with(|| AnnualTaxableAmounts {
                cash_interest: dec!(0.0),
                share_lending_interest: dec!(0.0),
                capital_gains: dec!(0.0),
                dividends: dec!(0.0),
                fx_appreciation: dec!(0.0),
                dividend_equivalents: dec!(0.0),
                capital_losses: dec!(0.0),
                withheld_tax_capital_gains: dec!(0.0),
                withheld_tax_dividends: dec!(0.0),
                withheld_tax_interest: dec!(0.0),
            })
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
    let fx_rate = event
        .applied_fx_rate
        .context("Missing FX rate for currency conversion")?;

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

    apply_taxation(ctx, tax_type, taxed_amount, withheld_tax)
}

fn calculate_taxable_values(
    event: &PortfolioEvent,
    ctx: &mut ProcessingContext<'_>,
    fx_rate: Decimal,
) -> Result<(Decimal, Decimal)> {
    let taxable_remainder = event.units * event.price_unit;
    let withheld_tax_percent = event
        .withholding_tax_percent
        .context("Missing withholding tax percentage")?;

    // e.g. for Belgian Tax (30%, used for cash interest by Wise, one can only offset up to 25% of
    // Austrian KESt)
    let remaining_withholding_tax_percent = match event.event_type {
        EventType::CashInterest => {
            if withheld_tax_percent > ctx.tax_rates.interest {
                ctx.tax_rates.interest
            } else {
                withheld_tax_percent
            }
        }
        EventType::ShareInterest => {
            if withheld_tax_percent > ctx.tax_rates.capital_gains {
                ctx.tax_rates.capital_gains
            } else {
                withheld_tax_percent
            }
        }
        EventType::Dividend => {
            if withheld_tax_percent > ctx.tax_rates.dividends {
                ctx.tax_rates.dividends
            } else {
                withheld_tax_percent
            }
        }
        _ => unreachable!(),
    };

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
            year_entry.withheld_tax_dividends += withheld_tax;
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

    if let Some(wht_percent) = event.withholding_tax_percent {
        let wht_percent_to_consider = wht_percent.min(ctx.tax_rates.capital_gains);
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

    let taxed_amount = {
        let fx_wac = ctx
            .currency_wacs
            .get_mut(&origin_currency)
            .context("Currency WAC not found for conversion")?;

        let eur_rate =
            convert_amount(dec!(1.0), &event.date.date_naive(), "EUR", &origin_currency).await?;
        let fx_delta = fx_wac.avg_rate - eur_rate;
        let taxed_amount = ((fx_delta / eur_rate) * event.units) / eur_rate;

        fx_wac.units -= event.units;
        if fx_wac.units < dec!(0.0) {
            fx_wac.units = dec!(0.0);
        }

        taxed_amount
    };

    ctx.get_year_entry().fx_appreciation += taxed_amount;

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

    let wacs = ctx
        .securities_wacs
        .entry(full_report.isin.clone())
        .or_insert(SecWac {
            units: dec!(0.0),
            average_cost: dec!(0.0),
            weighted_avg_fx_rate: dec!(1.0),
            name: full_report.isin.clone(),
        });

    let taxed_amount =
        (full_report.dividend_aequivalent + full_report.intermittent_dividends) * wacs.units;

    let taxed_eur = convert_amount(
        taxed_amount,
        &full_report.date.date_naive(),
        &full_report.currency,
        "EUR",
    )
    .await?;

    let withheld_tax = full_report.withheld_dividend * wacs.units;
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

    Ok(())
}

pub async fn get_capital_gains_tax_report() -> Result<TaxationReport> {
    info!(target: "tax_report", "Starting capital gains tax report generation");

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
        };

        let start_date = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).unwrap();
        let end_date = Utc.with_ymd_and_hms(year, 12, 31, 23, 59, 59).unwrap();
        let events = get_events(start_date, end_date).await?;
        for event in events {
            process_event(event, &mut ctx).await?;
        }
    }

    post_process(
        &mut taxable_amounts,
        &mut currency_wacs,
        &mut securities_wacs,
    );

    let report = TaxationReport {
        created_at: Utc::now(),
        taxable_amounts,
        securities_wacs,
        currency_wacs,
    };

    info!(target: "tax_report", "Exporting taxation report to JSON");
    export_json(&report, "taxation")?;
    info!(target: "tax_report", "Tax report generated successfully");

    Ok(report)
}

fn post_process(
    taxable_amounts: &mut BTreeMap<i32, AnnualTaxableAmounts>,
    currency_wacs: &mut BTreeMap<String, FxWac>,
    securities_wacs: &mut BTreeMap<String, SecWac>,
) {
    for amounts in taxable_amounts.values_mut() {
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
