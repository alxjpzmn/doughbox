use crate::{
    services::market_data::oekb::fetch_and_store_oekb_fund_report,
};

pub async fn update_oekb_fund_reports(isins: &[String]) -> anyhow::Result<()> {
    for isin in isins {
        if let Err(e) = fetch_and_store_oekb_fund_report(isin).await {
            println!("Failed to fetch OeKB fund reports for {}: {}", isin, e);
        }
    }
    Ok(())
}
