use tabled::Table;

use crate::services::taxation::get_capital_gains_tax_report;

pub async fn calculate_taxes() -> anyhow::Result<()> {
    let taxation_report = get_capital_gains_tax_report().await?;

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
