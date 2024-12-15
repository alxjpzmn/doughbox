use crate::{
    database::queries::composite::get_used_isins,
    services::market_data::oekb::fetch_and_store_oekb_fund_report,
};

pub async fn update_oekb_fund_reports() -> anyhow::Result<()> {
    let isins = get_used_isins().await?;
    for isin in isins {
        fetch_and_store_oekb_fund_report(&isin).await?;
    }
    Ok(())
}
