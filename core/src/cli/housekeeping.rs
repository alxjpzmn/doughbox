use crate::services::instruments::stock_splits::update_stock_splits;

pub async fn housekeeping() -> anyhow::Result<()> {
    update_stock_splits().await?;
    Ok(())
}
