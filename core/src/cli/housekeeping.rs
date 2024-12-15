use crate::services::instruments::{
    fund_data::update_oekb_fund_reports, stock_splits::update_stock_splits,
};

pub async fn housekeeping() -> anyhow::Result<()> {
    update_stock_splits().await?;
    update_oekb_fund_reports().await?;
    Ok(())
}
