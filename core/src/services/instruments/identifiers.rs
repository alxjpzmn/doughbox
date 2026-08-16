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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn change(from: &str, to: &str) -> ListingChange {
        ListingChange {
            id: "lc-1".to_string(),
            ex_date: chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            from_factor: dec!(1),
            to_factor: dec!(1),
            from_identifier: from.to_string(),
            to_identifier: to.to_string(),
        }
    }

    #[test]
    fn remaps_known_identifier() {
        let changes = vec![change("US0126531013", "US0126532011")];
        assert_eq!(
            get_changed_identifier("US0126531013", changes),
            "US0126532011"
        );
    }

    #[test]
    fn leaves_unknown_identifier_unchanged() {
        let changes = vec![change("US0126531013", "US0126532011")];
        assert_eq!(
            get_changed_identifier("US0378331005", changes),
            "US0378331005"
        );
    }
}
