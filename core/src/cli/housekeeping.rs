use crate::services::{
    instruments::{fund_data::update_oekb_fund_reports, stock_splits::update_stock_splits},
    market_data::fx_rates::fetch_historic_ecb_rates,
};

pub async fn housekeeping() -> anyhow::Result<()> {
    update_stock_splits().await?;
    update_oekb_fund_reports().await?;
    fetch_historic_ecb_rates().await?;
    Ok(())
}
