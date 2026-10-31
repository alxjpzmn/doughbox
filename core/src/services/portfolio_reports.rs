use std::fmt::{self, Write};

use chrono::Utc;
use num_format::{Buffer, Locale};
use rust_decimal::Decimal;

use crate::database::{
    models::performance::PerformanceSignal,
    queries::performance::get_latest_performance_signal,
};
use super::{notifications::Notification, portfolio::PortfolioOverview};

/// Validate before either publishing a report or persisting a performance
/// snapshot, so missing valuations never become authoritative totals.
pub fn complete_snapshot(overview: &PortfolioOverview) -> anyhow::Result<PerformanceSignal> {
    if let Some(gap) = overview.security_history_gaps.first() {
        return Err(gap.clone().into());
    }
    anyhow::ensure!(overview.valuation_complete,
        "Portfolio valuation is incomplete (missing prices: {}); not saving a snapshot or sending notifications",
        overview.unpriced_isins.join(", "));
    anyhow::ensure!(!overview.returns_incomplete,
        "Portfolio return is incomplete ({} custom assets lack valuation or cost data); not saving a snapshot or sending notifications",
        overview.unpriced_assets);
    Ok(PerformanceSignal {
        date: Utc::now(),
        total_value: overview.total_value,
        total_invested: overview.total_value - overview.total_return_abs,
        gross_invested: Some(overview.total_invested),
        components: Some(overview.performance_components.clone()),
    })
}

struct ReportAmount {
    amount: Decimal,
    signed: bool,
}

impl fmt::Display for ReportAmount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let euros = self.amount.round().mantissa();
        let sign = if euros < 0 {
            "−"
        } else if self.signed && euros > 0 {
            "+"
        } else {
            ""
        };
        let mut digits = Buffer::default();
        digits.write_formatted(&euros.abs(), &Locale::de);
        write!(formatter, "{sign}€{}", digits.as_str())
    }
}

pub fn format_report(current: &PerformanceSignal, previous: Option<&PerformanceSignal>) -> String {
    let mut text = String::with_capacity(512);
    write!(text,
        "<b>Portfolio · {}</b>\n\
         <i>{} UTC</i>\n\n\
         <b>{}</b>\n\
         Total P&amp;L: <b>{}</b>",
        current.date.format("%-d %b %Y"),
        current.date.format("%H:%M"),
        ReportAmount { amount: current.total_value, signed: false },
        ReportAmount { amount: current.total_value - current.total_invested, signed: true },
    ).expect("writing to a String cannot fail");
    if let Some(previous) = previous {
        let capital_flow = current.total_invested - previous.total_invested;
        let value_change = current.total_value - previous.total_value;
        write!(text,
            "\n\n<b>Since {} · {} UTC</b>\n\
             Portfolio change: <b>{}</b>\n\
             Net capital flow: {}\n\
             P&amp;L change: <b>{}</b>",
            previous.date.format("%-d %b %Y"),
            previous.date.format("%H:%M"),
            ReportAmount { amount: value_change, signed: true },
            ReportAmount { amount: capital_flow, signed: true },
            ReportAmount { amount: value_change - capital_flow, signed: true },
        ).expect("writing to a String cannot fail");
    }
    text
}

/// The first report still sends current values; historical deltas are optional.
/// Callers persist the snapshot only after a successful send, as in the CLI.
pub async fn send_report(current: &PerformanceSignal) -> anyhow::Result<()> {
    let previous = get_latest_performance_signal().await?;
    Notification { content: format_report(current, previous.as_ref()) }.send().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn report_amounts_group_whole_euros_and_sign_the_rounded_value() {
        for (amount, signed, expected) in [
            (dec!(123456.49), false, "€123.456"),
            (dec!(123456.51), true, "+€123.457"),
            (dec!(-123456.51), true, "−€123.457"),
            (dec!(-123456.51), false, "−€123.457"),
            (dec!(0), true, "€0"),
            (dec!(0.49), true, "€0"),
            (dec!(-0.49), true, "€0"),
            (dec!(0.5), true, "€0"),
            (dec!(-0.5), true, "€0"),
            (dec!(1.5), true, "+€2"),
            (dec!(-1.5), true, "−€2"),
            (Decimal::MAX, false, "€79.228.162.514.264.337.593.543.950.335"),
            (Decimal::MIN, true, "−€79.228.162.514.264.337.593.543.950.335"),
        ] {
            assert_eq!(ReportAmount { amount, signed }.to_string(), expected);
        }
    }

    #[test]
    fn incomplete_valuation_or_returns_cannot_create_snapshots() {
        let mut overview = PortfolioOverview {
            generated_at: 0, total_value: dec!(1200), realized: dec!(0),
            total_return_abs: dec!(200), total_return_rel: dec!(20),
            total_invested: dec!(1500),
            income: dec!(0), performance_components: vec![],
            returns_incomplete: false, unpriced_assets: 0, positions: vec![],
            valuation_complete: true, unpriced_isins: vec![],
            security_history_gaps: vec![],
        };
        let complete = complete_snapshot(&overview).unwrap();
        assert_eq!(complete.total_invested, dec!(1000));
        assert_eq!(complete.gross_invested, Some(dec!(1500)));
        overview.valuation_complete = false;
        assert!(complete_snapshot(&overview).is_err());
        overview.valuation_complete = true;
        overview.returns_incomplete = true;
        overview.unpriced_assets = 1;
        assert!(complete_snapshot(&overview).is_err());
        overview.returns_incomplete = false;
        overview.unpriced_assets = 0;
        overview.security_history_gaps.push(super::super::performance::SecurityHistoryGap {
            isin: "missing".into(), broker: "Test".into(), date: Utc::now(),
            available_units: dec!(2), sold_units: dec!(3),
        });
        let error = complete_snapshot(&overview).unwrap_err();
        let gap = error.downcast_ref::<super::super::performance::SecurityHistoryGap>().unwrap();
        assert_eq!(gap.available_units, dec!(2));
        assert_eq!(gap.sold_units, dec!(3));
    }

    #[test]
    fn snapshots_preserve_sub_cent_returns_and_contributions() {
        for (cost, value) in [(dec!(0.03), dec!(0.034)), (dec!(0.003), dec!(0.0034))] {
            let overview = PortfolioOverview {
                generated_at: 0, total_value: value, realized: dec!(0),
                total_return_abs: value - cost, total_return_rel: dec!(13.33),
                total_invested: cost,
                income: dec!(0), performance_components: vec![],
                returns_incomplete: false, unpriced_assets: 0, positions: vec![],
                valuation_complete: true, unpriced_isins: vec![],
                security_history_gaps: vec![],
            };
            let snapshot = complete_snapshot(&overview).unwrap();
            assert_eq!(snapshot.total_value, value);
            assert_eq!(snapshot.total_invested, cost);
            assert_eq!(snapshot.gross_invested, Some(cost));
            let history_return = (snapshot.total_value - snapshot.total_invested)
                / snapshot.gross_invested.unwrap() * dec!(100);
            assert_eq!(history_return.round_dp(2), dec!(13.33));

            let response = serde_json::to_value(&overview).unwrap();
            assert_eq!(response["total_value"], value.round_dp(2).to_string());
            assert_eq!(response["total_return_abs"], (value - cost).round_dp(2).to_string());
            assert_eq!(response["total_invested"], cost.round_dp(2).to_string());
        }
    }
}
