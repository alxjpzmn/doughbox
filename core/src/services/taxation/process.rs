use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;

use crate::database::models::fund_report::FundTaxReport;
use crate::database::queries::{composite::get_active_years, fund_report::get_oekb_fund_report_by_id, fx_rate::get_exchange_rate, tax_optimization::get_tax_optimizations_by_date_range};
use crate::services::dtt::{determine_source_country, treaty_rate, DttIncomeType};
use crate::services::events::{get_events, EventType, PortfolioEvent, TradeDirection};
use crate::database::queries::{listing_change::get_listing_changes, stock_split::get_stock_splits};
use crate::services::instruments::{identifiers::get_changed_identifier, stock_splits::get_split_adjusted_units};
use super::types::{AnnualTaxableAmounts, TaxRates, TaxationReport, TransactionTaxImpact};
use super::wac::{eur_unit_price, pool_key, SecWac};

pub(crate) fn tax_rates() -> TaxRates {
    TaxRates { interest: dec!(0.25), capital_gains: dec!(0.275), dividends: dec!(0.275) }
}

fn limitations() -> Vec<String> {
    [
        "Security WAC is fee-exclusive EUR cost per broker and ISIN. Broker is only a depot proxy: multiple depots at the same broker cannot be distinguished; complete acquisition history is required.",
        "Foreign-currency cash realization is unavailable (fx_appreciation=null, currency_wacs empty). Imports do not identify a complete cash ledger, opening EUR basis, automatic FX versus held cash, or interest-bearing account status. Security EUR gains already include their own FX movement; no second FX gain is added. Spending held FX on securities can separately realize cash gains and requires manual calculation.",
        "Income amounts are treated as gross, never grossed up by creditable withholding. Some legacy imports contain net amounts or inferred taxes (notably Wise/Revolut); reconcile source statements and historical FX before filing.",
        "Treaty credits are estimates using the supported country table and ISIN/broker source inference, which cannot establish issuer residence, beneficial ownership or treaty eligibility. Unknown source credits are not assumed; Austrian versus foreign withholding is not distinguished by stored data.",
        "Cash interest uses the 25% bank-deposit rate only as an estimate; broker cash, money-market funds and lending income require legal classification. Share-lending income uses 27.5% as an estimate, not an established classification.",
        "OeKB: only stored reporting-fund data is processed at the report inflow date. Field 10287 is dividend-equivalent income, 10289 adjusts EUR basis; 10595 is a warning, not income. Distribution dates, corrections/replacements, private-investor option column and complete report coverage require review. Broker distributions are not automatically reconciled to OeKB distribution fields.",
        "Tax liabilities are provisional before annual cross-category loss offset, assessment options and country-specific credit allocation; not final Austrian tax payable. Manual tax adjustments are annual information only and excluded from transaction income totals. Form labels are indicative and must be checked for the filing year.",
    ].into_iter().map(str::to_owned).collect()
}

pub(crate) fn in_range(date: DateTime<Utc>, from: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> bool {
    from.is_none_or(|d| date >= d) && until.is_none_or(|d| date <= d)
}

pub(crate) async fn load_events(until: Option<DateTime<Utc>>) -> Result<Vec<PortfolioEvent>> {
    let years = get_active_years().await?;
    let Some(first) = years.iter().min().copied() else { return Ok(Vec::new()); };
    let last = years.iter().max().copied().unwrap();
    let start = Utc.with_ymd_and_hms(first, 1, 1, 0, 0, 0).unwrap();
    // Include subsecond events at year end, without overlapping adjacent years.
    let end = match until {
        Some(until) => until.checked_add_signed(chrono::Duration::nanoseconds(1))
            .context("Tax report end date is out of range")?,
        None => Utc.with_ymd_and_hms(last + 1, 1, 1, 0, 0, 0).unwrap(),
    };
    if end <= start { return Ok(Vec::new()); }
    get_events(start, end).await
}

pub(crate) struct TaxEngine {
    pub(crate) amounts: BTreeMap<i32, AnnualTaxableAmounts>,
    pub(crate) securities: BTreeMap<String, SecWac>,
    rates: TaxRates,
}

impl TaxEngine {
    pub(crate) fn new() -> Self {
        Self { amounts: BTreeMap::new(), securities: BTreeMap::new(), rates: tax_rates() }
    }

    pub(crate) async fn process(&mut self, event: &PortfolioEvent, count: bool) -> Result<TransactionTaxImpact> {
        let mut impact = base_impact(event);
        if !event.tax_supported {
            impact.impact_type = "Excluded activity".into();
            impact.notes = "Activity is excluded from automated Austrian taxation; manual review required".into();
            return Ok(impact);
        }
        match event.event_type {
            EventType::Trade => self.trade(event, &mut impact)?,
            EventType::CashInterest | EventType::ShareInterest | EventType::Dividend => {
                let rate = if event.currency == "EUR" { dec!(1) } else if let Some(rate) = event.applied_fx_rate {
                    rate
                } else {
                    // Database conversion rates are EUR per native unit; event rates are the inverse.
                    let eur_per_native = get_exchange_rate(&event.currency, "EUR", &event.date.date_naive()).await?;
                    ensure!(eur_per_native > dec!(0), "Income FX rate must be positive");
                    dec!(1) / eur_per_native
                };
                self.income(event, rate, &mut impact)?;
            }
            EventType::DividendAequivalent => {
                let id = event.identifier.as_deref().context("Missing OeKB report ID")?.parse::<i32>()?;
                let mut report = get_oekb_fund_report_by_id(id).await?;
                // Trade events use globally split-adjusted units, so historical per-share
                // report values must be on that same share scale before touching basis.
                let mut splits = get_stock_splits().await?;
                let factor = get_split_adjusted_units(&report.isin, dec!(1), report.date, &mut splits);
                normalize_fund_shares(&mut report, factor)?;
                report.isin = get_changed_identifier(&report.isin, get_listing_changes().await?);
                let eur_per_native = if report.currency == "EUR" { dec!(1) } else {
                    get_exchange_rate(&report.currency, "EUR", &report.date.date_naive()).await?
                };
                self.fund(&report, eur_per_native, &mut impact)?;
            }
            EventType::FxConversion => {
                impact.impact_type = "FX cash: unavailable".into();
                impact.notes = "No cash-FX tax amount computed: complete cash balances, historical EUR basis, account classification and automatic-conversion identification are unavailable. Not a zero taxable gain.".into();
            }
            _ => {
                impact.impact_type = "Manual review".into();
                impact.notes = "No supported automated tax classification".into();
            }
        }
        if count { self.record(event, &impact); }
        Ok(impact)
    }

    fn trade(&mut self, event: &PortfolioEvent, impact: &mut TransactionTaxImpact) -> Result<()> {
        ensure!(event.units > dec!(0), "Trade units must be positive");
        let id = event.identifier.as_deref().context("Missing security identifier")?;
        let key = pool_key(&event.broker, id);
        match event.direction.as_ref().context("Missing trade direction")? {
            TradeDirection::Buy => {
                let wac = self.securities.entry(key).or_insert_with(|| SecWac {
                    units: dec!(0), average_cost: dec!(0), name: event.name.clone().unwrap_or_else(|| id.to_owned()),
                    broker: event.broker.clone(), identifier: id.to_owned(),
                });
                wac.update(event)?;
                impact.impact_type = "Security purchase".into();
                impact.notes = format!("Fee-exclusive EUR WAC: {}. Any disposal of previously held foreign cash is separate and unavailable.", wac.average_cost);
            }
            TradeDirection::Sell => {
                let price = eur_unit_price(event)?;
                let wac = self.securities.get_mut(&key).context("Missing acquisition history for this broker/depot and security")?;
                ensure!(event.units <= wac.units, "Sale exceeds known units for this broker/depot and security");
                impact.taxable_amount = event.units * (price - wac.average_cost);
                impact.recorded_withholding_tax = event.withholding_tax_percent.map(|p| p.max(dec!(0)) * event.units * price);
                impact.tax_rate_percent = self.rates.capital_gains * dec!(100);
                impact.tax_liability = impact.taxable_amount.max(dec!(0)) * self.rates.capital_gains;
                impact.impact_type = "Security disposal".into();
                impact.is_tax_relevant = true;
                impact.notes = format!("EUR proceeds {} minus historical EUR WAC basis {}. Fees excluded; embedded security FX included, not cash FX. Tax estimate before annual loss offset. Sale withholding is information only: stored data does not distinguish Austrian KESt from foreign tax.", event.units * price, event.units * wac.average_cost);
                wac.units -= event.units;
                if wac.units == dec!(0) { self.securities.remove(&key); }
            }
        }
        Ok(())
    }

    fn income(&self, event: &PortfolioEvent, native_per_eur: Decimal, impact: &mut TransactionTaxImpact) -> Result<()> {
        ensure!(native_per_eur > dec!(0), "Income FX rate must be positive");
        let (kind, rate, label) = match event.event_type {
            EventType::CashInterest => (DttIncomeType::Interest, self.rates.interest, "Cash interest (classification estimate)"),
            EventType::ShareInterest => (DttIncomeType::CapitalGains, self.rates.capital_gains, "Share lending (classification estimate)"),
            EventType::Dividend => (DttIncomeType::Dividends, self.rates.dividends, "Dividend"),
            _ => unreachable!(),
        };
        let gross = event.units * event.price_unit / native_per_eur;
        let source = determine_source_country(event.identifier.as_deref(), &event.broker);
        // Unknown treaty/source must not silently permit an Austrian-rate foreign credit.
        let cap = source.and_then(|country| treaty_rate(country, kind)).unwrap_or(dec!(0)).min(rate);
        let actual_percent = event.withholding_tax_percent.unwrap_or(dec!(0)).max(dec!(0));
        let credit = gross * actual_percent.min(cap);
        impact.impact_type = label.into();
        impact.taxable_amount = gross;
        impact.recorded_withholding_tax = event.withholding_tax_percent.map(|_| gross * actual_percent);
        impact.withheld_tax = credit;
        // Corrections reverse prior income and tax; floor only the annual liability.
        impact.tax_liability = gross * rate - credit;
        impact.tax_rate_percent = rate * dec!(100);
        impact.source_country = source.map(str::to_owned);
        impact.dtt_rate_percent = source.and_then(|country| treaty_rate(country, kind)).map(|r| r * dec!(100));
        impact.is_tax_relevant = true;
        impact.notes = format!("Gross income EUR {}; actual recorded withholding EUR {}; estimated credit EUR {}. Gross is not increased by withholding. Source/treaty eligibility and gross-versus-net import convention require review; liability before annual loss offset.", gross, gross * actual_percent, credit);
        Ok(())
    }

    fn fund(&mut self, report: &FundTaxReport, eur_per_native: Decimal, impact: &mut TransactionTaxImpact) -> Result<()> {
        ensure!(eur_per_native > dec!(0), "OeKB FX rate must be positive");
        let units: Decimal = self.securities.values().filter(|w| w.identifier == report.isin).map(|w| w.units).sum();
        if units == dec!(0) {
            impact.impact_type = "OeKB: no holdings".into();
            impact.notes = "No known units held at report inflow date".into();
            return Ok(());
        }
        ensure!(report.dividend_aequivalent >= dec!(0), "Negative OeKB dividend-equivalent income requires proof of holding at the earlier unreported distribution; manual review required");
        let adjustment = report.wac_adjustment * eur_per_native;
        for wac in self.securities.values_mut().filter(|w| w.identifier == report.isin) {
            ensure!(wac.average_cost + adjustment >= dec!(0), "OeKB correction would make acquisition basis negative");
            wac.average_cost += adjustment;
        }
        impact.identifier = Some(report.isin.clone());
        impact.units = units;
        impact.price_unit = report.dividend_aequivalent;
        impact.total = units * report.dividend_aequivalent;
        impact.total_currency = report.currency.clone();
        impact.taxable_amount = report.dividend_aequivalent * units * eur_per_native;
        // Field 10288 is the published assessment credit; never infer a treaty from fund domicile.
        // A distribution report may include credits on distributions: do not assign those to AGE.
        let credit = if report.dividend == dec!(0) && report.intermittent_dividends == dec!(0) {
            (report.withheld_dividend * units * eur_per_native).max(dec!(0)).min(impact.taxable_amount * self.rates.dividends)
        } else { dec!(0) };
        impact.withheld_tax = credit;
        impact.tax_rate_percent = self.rates.dividends * dec!(100);
        impact.tax_liability = (impact.taxable_amount * self.rates.dividends - credit).max(dec!(0));
        impact.impact_type = "Dividend equivalent (OeKB)".into();
        impact.is_tax_relevant = true;
        impact.notes = format!("{} units at report inflow date; field 10287 income only, EUR basis adjustment/share {} from 10289. Field 10595 is an unreported-distribution warning, and 10759 is loss-offset information, neither is added to AGE. Distribution dates/credits and report corrections require reconciliation; imported investor-option column requires review. Report KESt field 10105 is not substituted for assessment liability.", units, adjustment);
        Ok(())
    }

    fn record(&mut self, event: &PortfolioEvent, impact: &TransactionTaxImpact) {
        if !impact.is_tax_relevant { return; }
        let annual = self.amounts.entry(event.date.year()).or_default();
        match event.event_type {
            EventType::Trade => {
                annual.capital_gains += impact.taxable_amount.max(dec!(0));
                annual.capital_losses += (-impact.taxable_amount).max(dec!(0));
                annual.withheld_tax_capital_gains += impact.recorded_withholding_tax.unwrap_or(dec!(0));
            }
            EventType::Dividend => {
                annual.dividends += impact.taxable_amount;
                annual.withheld_tax_dividends += impact.withheld_tax;
                annual.tax_owed_dividends += impact.tax_liability;
            }
            EventType::CashInterest => {
                annual.cash_interest += impact.taxable_amount;
                annual.withheld_tax_interest += impact.withheld_tax;
            }
            EventType::ShareInterest => annual.share_lending_interest += impact.taxable_amount,
            EventType::DividendAequivalent => {
                annual.dividend_equivalents += impact.taxable_amount;
                annual.tax_owed_dividend_equivalents += impact.tax_liability;
            }
            _ => {}
        }
    }
}

fn normalize_fund_shares(report: &mut FundTaxReport, split_factor: Decimal) -> Result<()> {
    ensure!(split_factor > dec!(0), "OeKB split adjustment factor must be positive");
    report.dividend_aequivalent /= split_factor;
    report.wac_adjustment /= split_factor;
    report.withheld_dividend /= split_factor;
    Ok(())
}

fn base_impact(event: &PortfolioEvent) -> TransactionTaxImpact {
    TransactionTaxImpact {
        date: event.date, event_type: event.event_type.clone(), identifier: event.identifier.clone(), name: event.name.clone(),
        direction: event.direction.clone(), currency: event.currency.clone(), units: event.units, price_unit: event.price_unit,
        total: event.total, total_currency: event.total_currency.clone(), broker: event.broker.clone(), impact_type: String::new(),
        taxable_amount: dec!(0), withheld_tax: dec!(0), tax_liability: dec!(0), tax_rate_percent: dec!(0),
        recorded_withholding_tax: None,
        source_country: None, dtt_rate_percent: None, notes: String::new(), is_tax_relevant: false,
    }
}

pub(crate) async fn calculate_report(from: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> Result<(TaxationReport, Vec<PortfolioEvent>, Vec<TransactionTaxImpact>)> {
    ensure!(from.zip(until).is_none_or(|(f, u)| f <= u), "Tax report start must not exceed end");
    let events = load_events(until).await?;
    let mut engine = TaxEngine::new();
    let mut impacts = Vec::new();
    for event in &events {
        let count = in_range(event.date, from, until);
        let impact = engine.process(event, count).await.with_context(|| format!("Tax event {:?} on {}", event.event_id, event.date))?;
        if count { impacts.push(impact); }
    }
    for year in get_active_years().await? {
        let start = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).unwrap();
        if until.is_some_and(|u| start > u) { continue; }
        let end = Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).unwrap() - chrono::Duration::nanoseconds(1);
        for opt in get_tax_optimizations_by_date_range(start, end).await? {
            if !in_range(opt.date, from, until) || opt.amount == dec!(0) { continue; }
            let annual = engine.amounts.entry(opt.date.year()).or_default();
            // Signed manual tax cash-flow information; not inferred taxable income or treaty credit.
            annual.tax_optimization_adjustment -= opt.amount;
        }
    }
    for annual in engine.amounts.values_mut() {
        annual.net_capital_gains = annual.capital_gains - annual.capital_losses;
        annual.tax_owed_dividends = annual.tax_owed_dividends.max(dec!(0));
        annual.round_all(2);
    }
    let report = TaxationReport {
        created_at: Utc::now(), from_date: from, until_date: until, taxable_amounts: engine.amounts,
        securities_wacs: engine.securities, currency_wacs: BTreeMap::new(), limitations: limitations(),
    };
    Ok((report, events, impacts))
}


#[cfg(test)]
mod tests {
    use super::*;
    fn event(direction: TradeDirection, units: Decimal, price: Decimal, fx: Decimal, broker: &str) -> PortfolioEvent {
        PortfolioEvent {
            event_id: None, date: Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap(), event_type: EventType::Trade,
            identifier: Some("US0378331005".into()), name: Some("Apple".into()), unit_label: None, currency: "USD".into(),
            units, price_unit: price, direction: Some(direction), applied_fx_rate: Some(fx), withholding_tax_percent: None,
            total: units * price, total_currency: "USD".into(), broker: broker.into(), tax_supported: true,
        }
    }

    #[tokio::test]
    async fn historical_eur_wac_includes_security_fx_and_excludes_cash_overlap() {
        let mut engine = TaxEngine::new();
        engine.process(&event(TradeDirection::Buy, dec!(10), dec!(100), dec!(2), "A"), false).await.unwrap();
        engine.process(&event(TradeDirection::Buy, dec!(10), dec!(100), dec!(1), "A"), false).await.unwrap();
        assert_eq!(engine.securities[&pool_key("A", "US0378331005")].average_cost, dec!(75));
        let sale = engine.process(&event(TradeDirection::Sell, dec!(10), dec!(100), dec!(1), "A"), true).await.unwrap();
        assert_eq!(sale.taxable_amount, dec!(250));
        assert_eq!(engine.amounts[&2024].capital_gains, sale.taxable_amount);
        assert_eq!(engine.amounts[&2024].fx_appreciation, None);
        assert_eq!(engine.securities[&pool_key("A", "US0378331005")].units, dec!(10));
    }

    #[tokio::test]
    async fn brokers_are_separate_and_oversells_fail() {
        let mut engine = TaxEngine::new();
        engine.process(&event(TradeDirection::Buy, dec!(10), dec!(100), dec!(1), "A"), false).await.unwrap();
        engine.process(&event(TradeDirection::Buy, dec!(10), dec!(200), dec!(1), "B"), false).await.unwrap();
        let sale = engine.process(&event(TradeDirection::Sell, dec!(10), dec!(150), dec!(1), "A"), true).await.unwrap();
        assert_eq!(sale.taxable_amount, dec!(500));
        assert!(engine.process(&event(TradeDirection::Sell, dec!(11), dec!(150), dec!(1), "B"), true).await.is_err());
    }

    #[tokio::test]
    async fn dated_sales_reduce_basis_even_outside_income_window() {
        let mut engine = TaxEngine::new();
        engine.process(&event(TradeDirection::Buy, dec!(10), dec!(100), dec!(1), "A"), false).await.unwrap();
        engine.process(&event(TradeDirection::Sell, dec!(10), dec!(120), dec!(1), "A"), false).await.unwrap();
        engine.process(&event(TradeDirection::Buy, dec!(2), dec!(300), dec!(1), "A"), false).await.unwrap();
        let sale = engine.process(&event(TradeDirection::Sell, dec!(1), dec!(350), dec!(1), "A"), true).await.unwrap();
        assert_eq!(sale.taxable_amount, dec!(50));
        assert_eq!(engine.amounts[&2024].capital_gains, dec!(50));
    }

    #[tokio::test]
    async fn gross_dividend_is_not_grossed_up_by_treaty_credit() {
        let mut engine = TaxEngine::new();
        let mut dividend = event(TradeDirection::Buy, dec!(100), dec!(1), dec!(2), "A");
        dividend.event_type = EventType::Dividend;
        dividend.direction = None;
        dividend.withholding_tax_percent = Some(dec!(0.30));
        let impact = engine.process(&dividend, true).await.unwrap();
        assert_eq!(impact.taxable_amount, dec!(50));
        assert_eq!(impact.withheld_tax, dec!(7.5));
        assert_eq!(impact.tax_liability, dec!(6.25));
        assert_eq!(engine.amounts[&2024].dividends, impact.taxable_amount);
        assert_eq!(engine.amounts[&2024].withheld_tax_dividends, impact.withheld_tax);
        assert_eq!(engine.amounts[&2024].tax_owed_dividends, impact.tax_liability);
    }

    #[tokio::test]
    async fn dividend_reversals_cancel_income_credits_and_tax() {
        for withholding_percent in [dec!(0), dec!(0.30)] {
            let mut engine = TaxEngine::new();
            let mut dividend = event(TradeDirection::Buy, dec!(100), dec!(1), dec!(1), "A");
            dividend.event_type = EventType::Dividend;
            dividend.direction = None;
            dividend.currency = "EUR".into();
            dividend.withholding_tax_percent = Some(withholding_percent);
            let original = engine.process(&dividend, true).await.unwrap();
            dividend.units = dec!(-100);
            dividend.total = dec!(-100);
            let reversal = engine.process(&dividend, true).await.unwrap();
            assert_eq!(reversal.taxable_amount, -original.taxable_amount);
            assert_eq!(reversal.recorded_withholding_tax, original.recorded_withholding_tax.map(|tax| -tax));
            assert_eq!(reversal.withheld_tax, -original.withheld_tax);
            assert_eq!(reversal.tax_liability, -original.tax_liability);
            let annual = &engine.amounts[&2024];
            assert_eq!(annual.dividends, dec!(0));
            assert_eq!(annual.withheld_tax_dividends, dec!(0));
            assert_eq!(annual.tax_owed_dividends, dec!(0));

            // A corrected replacement must be taxed once, after cancelling the original.
            dividend.units = dec!(80);
            dividend.total = dec!(80);
            let replacement = engine.process(&dividend, true).await.unwrap();
            let annual = &engine.amounts[&2024];
            assert_eq!(annual.dividends, dec!(80));
            assert_eq!(annual.withheld_tax_dividends, replacement.withheld_tax);
            assert_eq!(annual.tax_owed_dividends, replacement.tax_liability);
        }
    }

    #[tokio::test]
    async fn missing_fx_is_not_treated_as_parity() {
        let mut engine = TaxEngine::new();
        let mut buy = event(TradeDirection::Buy, dec!(1), dec!(100), dec!(1), "A");
        buy.applied_fx_rate = None;
        assert!(engine.process(&buy, false).await.is_err());
    }

    #[tokio::test]
    async fn fund_adjustment_is_eur_and_warning_fields_are_not_income() {
        let mut engine = TaxEngine::new();
        let buy = event(TradeDirection::Buy, dec!(10), dec!(100), dec!(2), "A");
        engine.process(&buy, false).await.unwrap();
        let report = FundTaxReport {
            id: 1, date: buy.date, isin: buy.identifier.clone().unwrap(), currency: "USD".into(),
            dividend: dec!(0), dividend_aequivalent: dec!(4), intermittent_dividends: dec!(1),
            withheld_dividend: dec!(0), wac_adjustment: dec!(6), inlaendische_dividenden: dec!(9),
            kest_inlaendische_dividenden: dec!(2), kest_per_share: dec!(999),
        };
        let mut impact = base_impact(&buy);
        engine.fund(&report, dec!(0.5), &mut impact).unwrap();
        assert_eq!(impact.taxable_amount, dec!(20));
        assert_eq!(impact.tax_liability, dec!(5.5));
        assert_eq!(engine.securities[&pool_key("A", "US0378331005")].average_cost, dec!(53));
        let sale = engine.process(&event(TradeDirection::Sell, dec!(10), dec!(120), dec!(2), "A"), true).await.unwrap();
        assert_eq!(sale.taxable_amount, dec!(70));
    }

    #[tokio::test]
    async fn fund_historical_per_share_values_match_split_adjusted_holdings() {
        let mut engine = TaxEngine::new();
        let buy = event(TradeDirection::Buy, dec!(20), dec!(50), dec!(1), "A");
        engine.process(&buy, false).await.unwrap();
        let mut report = FundTaxReport {
            id: 1, date: buy.date, isin: buy.identifier.clone().unwrap(), currency: "EUR".into(),
            dividend: dec!(0), dividend_aequivalent: dec!(4), intermittent_dividends: dec!(0),
            withheld_dividend: dec!(1), wac_adjustment: dec!(6), inlaendische_dividenden: dec!(0),
            kest_inlaendische_dividenden: dec!(0), kest_per_share: dec!(0),
        };
        normalize_fund_shares(&mut report, dec!(2)).unwrap();
        let mut impact = base_impact(&buy);
        engine.fund(&report, dec!(1), &mut impact).unwrap();
        assert_eq!(impact.taxable_amount, dec!(40));
        assert_eq!(impact.withheld_tax, dec!(10));
        assert_eq!(engine.securities[&pool_key("A", "US0378331005")].average_cost, dec!(53));
    }

    #[tokio::test]
    async fn fees_in_cash_total_do_not_enter_security_tax_basis_or_proceeds() {
        let mut engine = TaxEngine::new();
        let mut buy = event(TradeDirection::Buy, dec!(1), dec!(100), dec!(1), "A");
        buy.total = dec!(110);
        engine.process(&buy, false).await.unwrap();
        let mut sell = event(TradeDirection::Sell, dec!(1), dec!(120), dec!(1), "A");
        sell.total = dec!(110);
        let impact = engine.process(&sell, true).await.unwrap();
        assert_eq!(impact.taxable_amount, dec!(20));
    }

    #[tokio::test]
    async fn unknown_treaty_source_does_not_receive_assumed_credit() {
        let mut engine = TaxEngine::new();
        let mut dividend = event(TradeDirection::Buy, dec!(100), dec!(1), dec!(1), "A");
        dividend.event_type = EventType::Dividend;
        dividend.identifier = Some("CH0000000000".into());
        dividend.withholding_tax_percent = Some(dec!(0.35));
        let impact = engine.process(&dividend, true).await.unwrap();
        assert_eq!(impact.withheld_tax, dec!(0));
        assert_eq!(impact.taxable_amount, dec!(100));
    }

    #[test]
    fn inclusive_range_preserves_subsecond_year_end() {
        let date = Utc.with_ymd_and_hms(2024, 12, 31, 23, 59, 59).unwrap() + chrono::Duration::milliseconds(999);
        assert!(in_range(date, Some(date), Some(date)));
        assert!(!in_range(date, None, Some(date - chrono::Duration::nanoseconds(1))));
    }
}
