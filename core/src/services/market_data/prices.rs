use rust_decimal::Decimal;

use crate::database::queries::instrument::get_instrument_by_id;

pub async fn get_current_instrument_price(isin: &str) -> anyhow::Result<Decimal> {
    let instrument = get_instrument_by_id(isin).await?;
    match instrument {
        Some(_) => Ok(instrument.unwrap().price),
        None => panic!("No price found for ISIN {} in instrument table.", isin),
    }
}
