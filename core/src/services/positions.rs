use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::database::{
    models::position::PositionWithName,
    queries::{instrument::batch_get_instrument_names, position::get_positions},
};

pub async fn get_positions_overview(
    date: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<PositionWithName>> {
    let positions = get_positions(date, None).await?;

    let isins: Vec<_> = positions
        .iter()
        .map(|position| position.isin.clone())
        .collect();

    let names = batch_get_instrument_names(&isins).await?;

    let name_map: HashMap<_, _> = isins.iter().zip(names.iter()).collect();

    let mut positions_with_name: Vec<PositionWithName> = positions
        .iter()
        .map(|position| {
            let name = name_map
                .get(&position.isin)
                .unwrap_or(&&position.isin)
                .to_string();
            PositionWithName {
                isin: position.isin.clone(),
                name,
                units: position.units,
            }
        })
        .collect();

    positions_with_name.sort_by(|a, b| b.name.partial_cmp(&a.name).unwrap());

    Ok(positions_with_name)
}
