use anyhow::{Context, Result};
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use log::{debug, info};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;

use crate::database::queries::tax_optimization::get_tax_optimizations_by_date_range;
use crate::database::queries::{
    composite::get_active_years, fund_report::get_oekb_fund_report_by_id,
    fx_rate::get_exchange_rate,
};
use crate::services::dtt::{determine_source_country, treaty_rate, DttIncomeType};
use crate::services::events::{get_events, EventType, PortfolioEvent, TradeDirection};
use crate::services::files::export_json;
use crate::services::market_data::fx_rates::convert_amount;

use super::types::{AnnualTaxableAmounts, TaxRates, TaxationReport};
use super::wac::{FxWac, SecWac};

pub(crate) struct ProcessingContext<'a> {
    taxable_amounts: &'a mut BTreeMap<i32, AnnualTaxableAmounts>,
    currency_wacs: &'a mut BTreeMap<String, FxWac>,
    securities_wacs: &'a mut BTreeMap<String, SecWac>,
    tax_rates: &'a TaxRates,
    year: i32,
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
}

impl ProcessingContext<'_> {
    pub(crate) fn get_year_entry(&mut self) -> &mut AnnualTaxableAmounts {
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
                tax_owed_dividends: dec!(0.0),
                tax_owed_dividend_equivalents: dec!(0.0),
            })
    }

    pub(crate) fn should_count_taxable(&self, event_date: DateTime<Utc>) -> bool {
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

pub(crate) async fn process_event(event: PortfolioEvent, ctx: &mut ProcessingContext<'_>) -> Result<()> {
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

pub(crate) fn calculate_taxable_values(
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

pub(crate) fn apply_taxation(
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
        sec_wac.units -= units;
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

pub(crate) fn process_eur_sell(
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
        let income_per_share = full_report.dividend_aequivalent
            + full_report.intermittent_dividends
            + full_report.inlaendische_dividenden;
        let income_amount = income_per_share * units_held;

        let income_eur = convert_amount(
            income_amount,
            &full_report.date.date_naive(),
            &full_report.currency,
            "EUR",
        )
        .await?;

        let kest_amount = full_report.kest_per_share * units_held;
        let kest_eur = convert_amount(
            kest_amount,
            &full_report.date.date_naive(),
            &full_report.currency,
            "EUR",
        )
        .await?;

        let year_entry = ctx.get_year_entry();
        year_entry.dividend_equivalents += income_eur;
        year_entry.tax_owed_dividend_equivalents += kest_eur;
    }

    Ok(())
}

pub async fn get_capital_gains_tax_report(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<TaxationReport> {
    info!(target: "tax_report", "Starting capital gains tax report generation (from={:?}, until={:?})", from_date, until_date);

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
                    year_entry.tax_owed_dividend_equivalents += opt.amount;
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

pub(crate) fn post_process(
    taxable_amounts: &mut BTreeMap<i32, AnnualTaxableAmounts>,
    currency_wacs: &mut BTreeMap<String, FxWac>,
    securities_wacs: &mut BTreeMap<String, SecWac>,
) {
    for amounts in taxable_amounts.values_mut() {
        amounts.net_capital_gains =
            (amounts.capital_gains - amounts.capital_losses).max(dec!(0.0));
        amounts.tax_owed_dividends =
            (amounts.dividends * dec!(0.275) - amounts.withheld_tax_dividends).max(dec!(0.0));
        amounts.tax_owed_dividend_equivalents = amounts.tax_owed_dividend_equivalents.max(dec!(0.0));
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn rates() -> TaxRates {
        TaxRates {
            interest: dec!(0.25),
            capital_gains: dec!(0.275),
            dividends: dec!(0.275),
        }
    }

    fn date(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
    }

    fn trade(
        direction: TradeDirection,
        units: Decimal,
        price: Decimal,
        when: DateTime<Utc>,
    ) -> PortfolioEvent {
        PortfolioEvent {
            date: when,
            event_type: EventType::Trade,
            currency: "EUR".to_string(),
            units,
            price_unit: price,
            identifier: Some("US0378331005".to_string()),
            name: Some("Apple".to_string()),
            direction: Some(direction),
            applied_fx_rate: Some(dec!(1)),
            withholding_tax_percent: None,
            total: units * price,
            broker: "Trading212".to_string(),
        }
    }

    fn dividend(amount: Decimal, wht: Decimal, isin: &str, broker: &str) -> PortfolioEvent {
        PortfolioEvent {
            date: date(2024, 6, 1),
            event_type: EventType::Dividend,
            currency: "EUR".to_string(),
            units: dec!(1),
            price_unit: amount,
            identifier: Some(isin.to_string()),
            name: Some("Fund".to_string()),
            direction: None,
            applied_fx_rate: Some(dec!(1)),
            withholding_tax_percent: Some(wht),
            total: amount,
            broker: broker.to_string(),
        }
    }

    #[test]
    fn date_window_filters_taxable_events() {
        let mut taxable_amounts = BTreeMap::new();
        let mut currency_wacs = BTreeMap::new();
        let mut securities_wacs = BTreeMap::new();
        let tax_rates = rates();
        let ctx = ProcessingContext {
            taxable_amounts: &mut taxable_amounts,
            currency_wacs: &mut currency_wacs,
            securities_wacs: &mut securities_wacs,
            tax_rates: &tax_rates,
            year: 2024,
            from_date: Some(date(2024, 3, 1)),
            until_date: Some(date(2024, 9, 1)),
        };
        assert!(ctx.should_count_taxable(date(2024, 6, 1)));
        assert!(!ctx.should_count_taxable(date(2024, 1, 1)));
        assert!(!ctx.should_count_taxable(date(2024, 12, 1)));
    }

    #[test]
    fn apply_taxation_splits_interest_and_dividends() {
        let mut taxable_amounts = BTreeMap::new();
        let mut currency_wacs = BTreeMap::new();
        let mut securities_wacs = BTreeMap::new();
        let tax_rates = rates();
        let mut ctx = ProcessingContext {
            taxable_amounts: &mut taxable_amounts,
            currency_wacs: &mut currency_wacs,
            securities_wacs: &mut securities_wacs,
            tax_rates: &tax_rates,
            year: 2024,
            from_date: None,
            until_date: None,
        };
        apply_taxation(&mut ctx, "interest", dec!(100), dec!(10)).unwrap();
        apply_taxation(&mut ctx, "dividends", dec!(80), dec!(12)).unwrap();
        let year = ctx.get_year_entry();
        assert_eq!(year.cash_interest, dec!(110));
        assert_eq!(year.withheld_tax_interest, dec!(10));
        assert_eq!(year.dividends, dec!(92));
        assert_eq!(year.withheld_tax_dividends, dec!(12));
    }

    #[test]
    fn eur_sell_records_gain_and_loss() {
        let mut taxable_amounts = BTreeMap::new();
        let mut currency_wacs = BTreeMap::new();
        let mut securities_wacs = BTreeMap::new();
        securities_wacs.insert(
            "US0378331005".to_string(),
            SecWac {
                units: dec!(10),
                average_cost: dec!(100),
                weighted_avg_fx_rate: dec!(1),
                name: "Apple".to_string(),
            },
        );
        let tax_rates = rates();
        let mut ctx = ProcessingContext {
            taxable_amounts: &mut taxable_amounts,
            currency_wacs: &mut currency_wacs,
            securities_wacs: &mut securities_wacs,
            tax_rates: &tax_rates,
            year: 2024,
            from_date: None,
            until_date: None,
        };
        process_eur_sell(trade(TradeDirection::Sell, dec!(4), dec!(150), date(2024, 6, 1)), &mut ctx, "US0378331005").unwrap();
        process_eur_sell(trade(TradeDirection::Sell, dec!(4), dec!(50), date(2024, 7, 1)), &mut ctx, "US0378331005").unwrap();
        let year = ctx.get_year_entry();
        assert_eq!(year.capital_gains, dec!(200));
        assert_eq!(year.capital_losses, dec!(200));
    }

    #[test]
    fn dtt_caps_us_dividend_withholding() {
        let mut taxable_amounts = BTreeMap::new();
        let mut currency_wacs = BTreeMap::new();
        let mut securities_wacs = BTreeMap::new();
        let tax_rates = rates();
        let mut ctx = ProcessingContext {
            taxable_amounts: &mut taxable_amounts,
            currency_wacs: &mut currency_wacs,
            securities_wacs: &mut securities_wacs,
            tax_rates: &tax_rates,
            year: 2024,
            from_date: None,
            until_date: None,
        };
        let event = dividend(dec!(100), dec!(0.30), "US0378331005", "Trading212");
        let (taxed, withheld) = calculate_taxable_values(&event, &mut ctx, dec!(1)).unwrap();
        assert_eq!(taxed, dec!(100));
        assert_eq!(withheld, dec!(15));
    }

    #[test]
    fn post_process_nets_gains_and_drops_empty_wacs() {
        let mut amounts = BTreeMap::new();
        amounts.insert(2024, {
            let mut a = AnnualTaxableAmounts::zero();
            a.capital_gains = dec!(500);
            a.capital_losses = dec!(200);
            a.dividends = dec!(100);
            a.withheld_tax_dividends = dec!(10);
            a
        });
        let mut currency_wacs = BTreeMap::from([(
            "USD".to_string(),
            FxWac {
                units: dec!(0),
                avg_rate: dec!(1.1),
            },
        )]);
        let mut securities_wacs = BTreeMap::from([(
            "US0378331005".to_string(),
            SecWac {
                units: dec!(2),
                average_cost: dec!(10.126),
                weighted_avg_fx_rate: dec!(1.119),
                name: "Apple".to_string(),
            },
        )]);
        post_process(&mut amounts, &mut currency_wacs, &mut securities_wacs);
        let year = amounts.get(&2024).unwrap();
        assert_eq!(year.net_capital_gains, dec!(300));
        assert_eq!(year.tax_owed_dividends, dec!(17.50));
        assert!(currency_wacs.is_empty());
        assert_eq!(securities_wacs["US0378331005"].average_cost, dec!(10.13));
    }

    #[tokio::test]
    async fn buy_then_sell_produces_kz_capital_gain() {
        let mut taxable_amounts = BTreeMap::new();
        let mut currency_wacs = BTreeMap::new();
        let mut securities_wacs = BTreeMap::new();
        let tax_rates = rates();
        let mut ctx = ProcessingContext {
            taxable_amounts: &mut taxable_amounts,
            currency_wacs: &mut currency_wacs,
            securities_wacs: &mut securities_wacs,
            tax_rates: &tax_rates,
            year: 2024,
            from_date: None,
            until_date: None,
        };

        process_event(
            trade(TradeDirection::Buy, dec!(10), dec!(100), date(2024, 1, 10)),
            &mut ctx,
        )
        .await
        .unwrap();
        process_event(
            trade(TradeDirection::Sell, dec!(10), dec!(150), date(2024, 8, 10)),
            &mut ctx,
        )
        .await
        .unwrap();
        process_event(
            dividend(dec!(40), dec!(0.15), "US0378331005", "Trading212"),
            &mut ctx,
        )
        .await
        .unwrap();

        post_process(&mut taxable_amounts, &mut currency_wacs, &mut securities_wacs);
        let year = taxable_amounts.get(&2024).unwrap();
        assert_eq!(year.capital_gains, dec!(500));
        assert_eq!(year.capital_losses, dec!(0));
        assert_eq!(year.net_capital_gains, dec!(500));
        assert_eq!(year.dividends, dec!(46));
        assert_eq!(year.withheld_tax_dividends, dec!(6));
        assert!(securities_wacs.is_empty());
    }
}
