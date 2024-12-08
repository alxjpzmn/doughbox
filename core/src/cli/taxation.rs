use spinners_rs::{Spinner, Spinners};
use tabled::Table;

use crate::services::{files::export_json, taxation::get_capital_gains_tax_report};

pub async fn calculate_taxes() -> anyhow::Result<()> {
    let mut sp = Spinner::new(Spinners::Point, "Calculating taxes");
    sp.start();

    let taxation_report = get_capital_gains_tax_report().await?;

    let taxable_amounts_table = Table::new(&taxation_report.taxable_amounts).to_string();

    export_json(&taxation_report, "taxation")?;

    sp.stop();
    println!("Taxable amounts:");
    println!("{}", taxable_amounts_table);
    println!("=========================");
    println!("Currency WAC:");
    println!("{:?}", taxation_report.currency_wacs);
    println!("=========================");
    println!("Securities WAC:");
    println!("{:?}", taxation_report.securities_wacs);

    Ok(())
}
