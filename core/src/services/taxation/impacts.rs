use anyhow::{Context, Result};
use chrono::Datelike;
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use log::info;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::{BTreeMap, HashSet};

use crate::database::queries::{
    composite::get_active_years, fund_report::get_oekb_fund_report_by_id,
    fx_rate::get_exchange_rate,
};
use crate::services::dtt::{self, determine_source_country, treaty_rate, DttIncomeType};
use crate::services::events::{get_events, EventType, PortfolioEvent, TradeDirection};
use crate::services::files::export_json;
use crate::services::market_data::fx_rates::convert_amount;
use crate::services::shared::constants::OUT_DIR;

use super::process::get_capital_gains_tax_report;
use super::types::{DetailedTaxationReport, TaxRates, TaxReportMetadata, TransactionTaxImpact};
use super::wac::{FxWac, SecWac};

struct ImpactContext {
    currency_wacs: BTreeMap<String, FxWac>,
    securities_wacs: BTreeMap<String, SecWac>,
    tax_rates: TaxRates,
}

impl ImpactContext {
    fn new(tax_rates: TaxRates) -> Self {
        Self {
            currency_wacs: BTreeMap::new(),
            securities_wacs: BTreeMap::new(),
            tax_rates,
        }
    }
}

pub async fn get_transaction_tax_impacts(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<Vec<TransactionTaxImpact>> {
    info!(target: "tax_report", "Starting transaction tax impact generation");

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

    let mut ctx = ImpactContext::new(tax_rates);
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
                        total_currency: event.total_currency.clone(),
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
    if !event.tax_supported {
        return Ok(TransactionTaxImpact {
            date: event.date,
            event_type: event.event_type.clone(),
            identifier: event.identifier.clone(),
            name: event.name.clone(),
            direction: event.direction.clone(),
            currency: event.currency.clone(),
            units: event.units,
            price_unit: event.price_unit,
            total: event.total,
            total_currency: event.total_currency.clone(),
            broker: event.broker.clone(),
            impact_type: "Excluded custom asset".to_string(),
            taxable_amount: dec!(0),
            withheld_tax: dec!(0),
            tax_liability: dec!(0),
            tax_rate_percent: dec!(0),
            source_country: None,
            dtt_rate_percent: None,
            notes: "Custom asset activity is excluded from automated taxation".to_string(),
            is_tax_relevant: false,
        });
    }

    match event.event_type {
        EventType::CashInterest | EventType::ShareInterest | EventType::Dividend => {
            compute_interest_or_dividend_impact(event, ctx).await
        }
        EventType::Trade => compute_trade_impact(event, ctx).await,
        EventType::FxConversion => compute_fx_conversion_impact(event, ctx).await,
        EventType::DividendAequivalent => compute_dividend_aequivalent_impact(event, ctx).await,
        EventType::Deposit
        | EventType::Withdrawal
        | EventType::PrincipalAdvance
        | EventType::PrincipalRepayment
        | EventType::OpeningBalance
        | EventType::BalanceReconciliation
        | EventType::PrivateDebtInterest
        | EventType::Valuation => unreachable!("custom asset events return above"),
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
            (capped_wht_percent * taxable_remainder) / fx_rate,
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
        total_currency: event.total_currency.clone(),
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
        total_currency: event.total_currency.clone(),
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
        sec_wac.units -= units;
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
            wht_amount / event.applied_fx_rate.unwrap_or(dec!(1.0))
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
        total_currency: event.total_currency.clone(),
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
                total_currency: event.total_currency.clone(),
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
                total_currency: event.total_currency.clone(),
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

    let notes = format!(
        "Fund report for {} units of {}. Income/share: {} {}, KESt/share: {} {}, KESt owed: {} EUR",
        units_held,
        full_report.isin,
        format_currency_value(income_per_share),
        full_report.currency,
        format_currency_value(full_report.kest_per_share),
        full_report.currency,
        format_currency_value(kest_eur),
    );

    Ok(TransactionTaxImpact {
        date: event.date,
        event_type: event.event_type.clone(),
        identifier: Some(full_report.isin.clone()),
        name: event.name.clone(),
        direction: event.direction.clone(),
        currency: full_report.currency.clone(),
        units: units_held,
        price_unit: income_per_share,
        total: event.total,
        total_currency: event.total_currency.clone(),
        broker: event.broker.clone(),
        impact_type: "Dividend Equivalent".to_string(),
        taxable_amount: income_eur.round_dp(2),
        withheld_tax: dec!(0.0),
        tax_liability: kest_eur.round_dp(2),
        tax_rate_percent: (ctx.tax_rates.dividends * dec!(100)).round_dp(1),
        source_country: dtt::isin_to_country(&full_report.isin).map(|c| c.to_string()),
        dtt_rate_percent: dtt::treaty_rate(
            dtt::isin_to_country(&full_report.isin).unwrap_or(""),
            DttIncomeType::Dividends,
        )
        .map(|r| (r * dec!(100)).round_dp(1)),
        notes,
        is_tax_relevant: income_eur != dec!(0.0),
    })
}

fn format_currency_value(value: Decimal) -> String {
    format!("{:.2}", value)
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
                if event.tax_supported
                    && (event.event_type == EventType::Trade
                        || event.event_type == EventType::DividendAequivalent)
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
