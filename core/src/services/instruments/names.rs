use crate::util::db_helpers::get_instrument_by_id;

pub async fn get_current_instrument_name(isin: &str) -> anyhow::Result<String> {
    let instrument = get_instrument_by_id(isin).await?;
    match instrument {
        Some(_) => Ok(instrument.unwrap().name),
        None => Ok("Unknown".to_string()),
    }
}
