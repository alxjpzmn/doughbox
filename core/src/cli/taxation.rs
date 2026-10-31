use chrono::{NaiveDate, TimeZone, Utc};
use tabled::Table;

use crate::services::taxation::{
    detailed_taxation_filename, export_detailed_capital_gains_tax_report,
    get_detailed_capital_gains_tax_report, publish_taxation_report,
};

pub async fn calculate_taxes(
    from_date: Option<NaiveDate>,
    until_date: Option<NaiveDate>,
) -> anyhow::Result<()> {
    let from = from_date.map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()));
    let until = until_date.map(|d| Utc.from_utc_datetime(&d.and_hms_nano_opt(23, 59, 59, 999_999_999).unwrap()));

    let detailed_report = get_detailed_capital_gains_tax_report(from, until).await?;
    publish_taxation_report(&detailed_report)?;
    let taxation_report = &detailed_report.report;

    let taxable_amounts_table = Table::new(&taxation_report.taxable_amounts).to_string();
    let securities_wac_table = Table::new(&taxation_report.securities_wacs).to_string();
    for limitation in &taxation_report.limitations {
        println!("LIMITATION: {}", limitation);
    }

    println!("Taxable amounts:");
    println!("{}", taxable_amounts_table);
    println!("Securities EUR WAC by brokerage portfolio:");
    println!("{}", securities_wac_table);
    println!("Foreign-currency cash gains / WAC: unavailable; not zero (see limitations).");
    println!();
    println!("E1kv indicative labels: [KZ 465] interest, [KZ 731/732] capital gains/losses, [KZ 897/898] dividends,");
    println!("  [KZ 936/937] dividend equivalents, [KZ 984/998] foreign WHT credits.");
    println!("  [info] fields are for reconciliation only — not entered on the form.");
    println!("  The form expects income amounts (brutto), not calculated tax.");
    println!("  KESt already withheld is reported by the broker via Steuerbescheinigung.");
    println!();
    println!("Treaty credits are estimates for supported sources; unknown source receives no assumed credit.");
    println!("     Per-transaction DTT details are included in the detailed JSON export.");
    println!(
        "Detailed taxation report with events exported to output/{}.json",
        detailed_taxation_filename(from_date, until_date)
    );
    Ok(())
}

pub async fn calculate_taxes_detailed(
    from_date: Option<NaiveDate>,
    until_date: Option<NaiveDate>,
) -> anyhow::Result<()> {
    let from = from_date.map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()));
    let until = until_date.map(|d| Utc.from_utc_datetime(&d.and_hms_nano_opt(23, 59, 59, 999_999_999).unwrap()));

    export_detailed_capital_gains_tax_report(from, until).await?;

    println!(
        "Detailed taxation report with events exported to output/{}.json",
        detailed_taxation_filename(from_date, until_date)
    );

    Ok(())
}
