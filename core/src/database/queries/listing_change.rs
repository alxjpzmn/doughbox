use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::database::{db_client, models::listing_change::ListingChange};

pub async fn get_listing_changes() -> anyhow::Result<Vec<ListingChange>> {
    let client = db_client().await?;

    let rows = client.query(r#"select * from listing_change"#, &[]).await?;

    let mut listing_changes: Vec<ListingChange> = vec![];

    for row in rows {
        let listing_change = ListingChange {
            id: row.get::<usize, String>(0),
            ex_date: row.get::<usize, DateTime<Utc>>(1),
            from_factor: row.get::<usize, Decimal>(2),
            to_factor: row.get::<usize, Decimal>(3),
            from_identifier: row.get::<usize, String>(4),
            to_identifier: row.get::<usize, String>(5),
        };

        listing_changes.push(listing_change);
    }

    Ok(listing_changes)
}
