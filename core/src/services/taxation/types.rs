use chrono::{DateTime, Utc};
use log::trace;
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeMap;
use tabled::Tabled;
use typeshare::typeshare;

use super::wac::{FxWac, SecWac};
use crate::services::events::{EventType, PortfolioEvent, TradeDirection};

#[typeshare]
#[derive(Debug, Serialize, Tabled)]
pub struct AnnualTaxableAmounts {
    #[tabled(rename = "Cash Interest [KZ 465]")]
    pub(crate) cash_interest: Decimal,
    #[tabled(rename = "Share Lending Interest [KZ 897/898]")]
    pub(crate) share_lending_interest: Decimal,
    #[tabled(rename = "Capital Gains [KZ 731]")]
    pub(crate) capital_gains: Decimal,
    #[tabled(rename = "Capital Losses [KZ 732]")]
    pub(crate) capital_losses: Decimal,
    #[tabled(rename = "Net Capital Gains")]
    pub(crate) net_capital_gains: Decimal,
    #[tabled(rename = "Dividends [KZ 897/898]")]
    pub(crate) dividends: Decimal,
    #[tabled(rename = "Dividend Equivalents [KZ 936/937]")]
    pub(crate) dividend_equivalents: Decimal,
    #[tabled(rename = "FX Appreciation [KZ 731]")]
    pub(crate) fx_appreciation: Decimal,
    #[tabled(rename = "WHT Capital Gains [info]")]
    pub(crate) withheld_tax_capital_gains: Decimal,
    #[tabled(rename = "WHT Dividends [KZ 984/998]")]
    pub(crate) withheld_tax_dividends: Decimal,
    #[tabled(rename = "WHT Interest [info]")]
    pub(crate) withheld_tax_interest: Decimal,
    #[tabled(rename = "Tax Optimization Adj. [info]")]
    pub(crate) tax_optimization_adjustment: Decimal,
    #[tabled(rename = "Tax Owed Dividends [info]")]
    pub(crate) tax_owed_dividends: Decimal,
    #[tabled(rename = "Tax Owed Div. Equivalents [info]")]
    pub(crate) tax_owed_dividend_equivalents: Decimal,
}

impl AnnualTaxableAmounts {
    pub(crate) fn zero() -> Self {
        Self {
            cash_interest: rust_decimal_macros::dec!(0),
            share_lending_interest: rust_decimal_macros::dec!(0),
            capital_gains: rust_decimal_macros::dec!(0),
            capital_losses: rust_decimal_macros::dec!(0),
            net_capital_gains: rust_decimal_macros::dec!(0),
            dividends: rust_decimal_macros::dec!(0),
            dividend_equivalents: rust_decimal_macros::dec!(0),
            fx_appreciation: rust_decimal_macros::dec!(0),
            withheld_tax_capital_gains: rust_decimal_macros::dec!(0),
            withheld_tax_dividends: rust_decimal_macros::dec!(0),
            withheld_tax_interest: rust_decimal_macros::dec!(0),
            tax_optimization_adjustment: rust_decimal_macros::dec!(0),
            tax_owed_dividends: rust_decimal_macros::dec!(0),
            tax_owed_dividend_equivalents: rust_decimal_macros::dec!(0),
        }
    }

    pub(crate) fn round_all(&mut self, dp: u32) {
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
            &mut self.tax_owed_dividends,
            &mut self.tax_owed_dividend_equivalents,
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

#[derive(Debug, Serialize)]
pub struct TaxRates {
    pub interest: Decimal,
    pub capital_gains: Decimal,
    pub dividends: Decimal,
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
