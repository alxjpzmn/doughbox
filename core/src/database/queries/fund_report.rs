use crate::database::{db_client, models::fund_report::FundTaxReport};

pub async fn get_fund_report_by_id(id: i32) -> anyhow::Result<FundTaxReport> {
    let client = db_client().await?;

    let row = client
        .query_one("SELECT * FROM fund_reports WHERE id = $1", &[&id])
        .await?;

    FundTaxReport::from_row(&row)
}

pub async fn add_fund_report_to_db(report: FundTaxReport) -> anyhow::Result<()> {
    let client = db_client().await?;

    client.execute(
        "INSERT INTO fund_reports (id, date, isin, currency, dividend, dividend_aequivalent, intermittent_dividend, withheld_dividend, wac_adjustment) values ($1, $2, $3, $4, $5, $6,$7, $8, $9) ON CONFLICT(id) DO NOTHING",
        &[&report.id, &report.date, &report.isin, &report.currency, &report.dividend, &report.dividend_aequivalent, &report.intermittent_dividends, &report.withheld_dividend, &report.wac_adjustment])
    .await?;

    Ok(())
}
