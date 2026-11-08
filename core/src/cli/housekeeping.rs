use crate::database::{models::auth::UserId, queries::composite::{get_used_currencies, get_used_isins}};
use crate::services::{
    instruments::{fund_data::update_oekb_fund_reports, stock_splits::update_stock_splits},
    market_data::fx_rates::fetch_historic_ecb_rates,
};

pub async fn housekeeping(user_id: &UserId) -> anyhow::Result<()> {
    let isins = get_used_isins(user_id).await?;
    update_stock_splits(&isins).await?;
    update_oekb_fund_reports(&isins).await?;
    for currency in get_used_currencies(user_id).await? {
        fetch_historic_ecb_rates(Some(&currency)).await?;
    }
    Ok(())
}
