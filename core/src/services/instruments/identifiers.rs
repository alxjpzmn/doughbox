use crate::database::models::listing_change::ListingChange;

pub fn get_changed_identifier(identifier: &str, listing_changes: Vec<ListingChange>) -> String {
    let relevant_changes = listing_changes
        .iter()
        .find(|item| item.from_identifier == *identifier);

    match relevant_changes {
        Some(listing_change) => (*listing_change).clone().to_identifier,
        None => identifier.to_string(),
    }
}
