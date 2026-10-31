use anyhow::Result;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::collections::{BTreeMap, BTreeSet};

use crate::services::files::export_json;
use super::process::{calculate_report, in_range, tax_rates};
use super::types::{DetailedTaxationReport, TaxReportMetadata};

pub async fn get_detailed_capital_gains_tax_report(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<DetailedTaxationReport> {
    // One historical replay supplies both annual values and transaction impacts.
    // Failures propagate: an incomplete report is never replaced with zero-valued impacts.
    let (report, events, transaction_impacts) = calculate_report(from_date, until_date).await?;
    let mut events_by_year = BTreeMap::new();
    let mut securities = BTreeSet::new();
    let mut currencies = BTreeSet::new();
    let mut earliest = None;
    let mut latest = None;
    let mut total = 0;
    for event in events.into_iter().filter(|e| in_range(e.date, from_date, until_date)) {
        earliest = Some(earliest.map_or(event.date, |d: DateTime<Utc>| d.min(event.date)));
        latest = Some(latest.map_or(event.date, |d: DateTime<Utc>| d.max(event.date)));
        if event.tax_supported && event.event_type == crate::services::events::EventType::Trade {
            if let Some(id) = &event.identifier { securities.insert(id.clone()); }
        }
        currencies.insert(event.currency.clone());
        total += 1;
        events_by_year.entry(event.date.year()).or_insert_with(Vec::new).push(event);
    }
    Ok(DetailedTaxationReport {
        report,
        tax_rates: tax_rates(),
        events_by_year,
        metadata: TaxReportMetadata {
            earliest_event_date: earliest, latest_event_date: latest, total_events: total,
            unique_securities: securities.into_iter().collect(), unique_currencies: currencies.into_iter().collect(),
        },
        transaction_impacts,
    })
}

/// The filename stem for the exact requested range; unbounded sides are explicit.
pub fn detailed_taxation_filename(
    from_date: Option<NaiveDate>,
    until_date: Option<NaiveDate>,
) -> String {
    if from_date.is_none() && until_date.is_none() {
        return "taxation_detailed".into();
    }
    let bound = |date: Option<NaiveDate>| {
        date.map_or_else(|| "all".into(), |date| date.format("%Y-%m-%d").to_string())
    };
    format!("taxation_detailed_from_{}_until_{}", bound(from_date), bound(until_date))
}

/// Publish every cached view from the same replay. Ranged reports never replace
/// the all-history summary or detailed bundle.
pub fn publish_taxation_report(report: &DetailedTaxationReport) -> Result<()> {
    let from_date = report.report.from_date.map(|date| date.date_naive());
    let until_date = report.report.until_date.map(|date| date.date_naive());
    export_json(report, &detailed_taxation_filename(from_date, until_date))?;
    if from_date.is_none() && until_date.is_none() {
        export_json(&report.report, "taxation")?;
    }
    Ok(())
}

pub async fn export_detailed_capital_gains_tax_report(
    from_date: Option<DateTime<Utc>>,
    until_date: Option<DateTime<Utc>>,
) -> Result<()> {
    let report = get_detailed_capital_gains_tax_report(from_date, until_date).await?;
    publish_taxation_report(&report)
}

#[cfg(test)]
mod tests {
    use super::detailed_taxation_filename;
    use chrono::NaiveDate;

    #[test]
    fn full_history_keeps_the_existing_detailed_filename() {
        assert_eq!(detailed_taxation_filename(None, None), "taxation_detailed");
    }

    #[test]
    fn bounded_and_open_ranges_have_distinct_filenames() {
        let from = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let until = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
        assert_eq!(
            detailed_taxation_filename(Some(from), Some(until)),
            "taxation_detailed_from_2024-01-01_until_2024-12-31"
        );
        assert_eq!(
            detailed_taxation_filename(Some(from), None),
            "taxation_detailed_from_2024-01-01_until_all"
        );
        assert_eq!(
            detailed_taxation_filename(None, Some(until)),
            "taxation_detailed_from_all_until_2024-12-31"
        );
    }
}
