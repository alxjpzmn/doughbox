use chrono::{NaiveDate, TimeZone, Utc};
use tabled::Table;

use crate::services::taxation::{
    export_detailed_capital_gains_tax_report, get_capital_gains_tax_report,
};

pub async fn calculate_taxes(
    from_date: Option<NaiveDate>,
    until_date: Option<NaiveDate>,
) -> anyhow::Result<()> {
    let from = from_date.map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()));
    let until = until_date.map(|d| Utc.from_utc_datetime(&d.and_hms_opt(23, 59, 59).unwrap()));

    let taxation_report = get_capital_gains_tax_report(from, until).await?;

    let taxable_amounts_table = Table::new(&taxation_report.taxable_amounts).to_string();
    let securities_wac_table = Table::new(&taxation_report.securities_wacs).to_string();
    let currency_wac_table = Table::new(&taxation_report.currency_wacs).to_string();

    println!("Taxable amounts:");
    println!("{}", taxable_amounts_table);
    println!("Securities WAC:");
    println!("{}", securities_wac_table);
    println!("Currency WAC:");
    println!("{}", currency_wac_table);

    Ok(())
}

pub async fn calculate_taxes_detailed(
    from_date: Option<NaiveDate>,
    until_date: Option<NaiveDate>,
) -> anyhow::Result<()> {
    let from = from_date.map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()));
    let until = until_date.map(|d| Utc.from_utc_datetime(&d.and_hms_opt(23, 59, 59).unwrap()));

    export_detailed_capital_gains_tax_report(from, until).await?;

    println!("Detailed taxation report with events exported to output/taxation_detailed.json");

    Ok(())
}
