use std::ffi::OsString;

use chrono::{TimeZone, Utc};
use rust_decimal_macros::dec;
use tokio_postgres::{Client, NoTls};

use super::*;
use crate::services::assets;

// The complete ignored database suite runs single-threaded. Each fixture owns its
// schema and temporarily points fresh service connections at that schema only.
struct Database {
    client: Client,
    schema: String,
    previous_url: Option<OsString>,
    owner: UserId,
    other: UserId,
}

impl Database {
    async fn new() -> Self {
        let test_url = std::env::var("TEST_POSTGRES_URL")
            .expect("Set TEST_POSTGRES_URL to an isolated PostgreSQL instance");
        let (mut client, connection) = tokio_postgres::connect(&test_url, NoTls).await.unwrap();
        tokio::spawn(async move { connection.await.unwrap() });
        let schema = format!("asset_isolation_{}", uuid::Uuid::new_v4().simple());
        client
            .batch_execute(&format!("CREATE SCHEMA {schema}; SET search_path TO {schema}"))
            .await
            .unwrap();
        refinery::embed_migrations!("migrations");
        migrations::runner().run_async(&mut client).await.unwrap();
        let owner = UserId::from_database(
            client
                .query_one("SELECT legacy_owner_user_id FROM auth_instance", &[])
                .await
                .unwrap()
                .get(0),
        );
        let other = UserId::new();
        client
            .execute(
                "INSERT INTO auth_users (id, role, status) VALUES ($1, 'member', 'pending')",
                &[&other.as_str()],
            )
            .await
            .unwrap();
        let mut scoped_url = reqwest::Url::parse(&test_url).unwrap();
        let options = scoped_url
            .query_pairs()
            .find(|(key, _)| key == "options")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        let pairs: Vec<_> = scoped_url
            .query_pairs()
            .filter(|(key, _)| key != "options")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        scoped_url.set_query(None);
        scoped_url
            .query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("options", &format!("{options} -csearch_path={schema}"));
        // tokio-postgres decodes percent escapes, not form-encoded '+' spaces.
        let query = scoped_url.query().unwrap().replace('+', "%20");
        scoped_url.set_query(Some(&query));
        let previous_url = std::env::var_os("POSTGRES_URL");
        std::env::set_var("POSTGRES_URL", scoped_url.as_str());
        Self { client, schema, previous_url, owner, other }
    }

    async fn cleanup(&self) {
        self.client
            .batch_execute(&format!("SET search_path TO public; DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if let Some(url) = &self.previous_url {
            std::env::set_var("POSTGRES_URL", url);
        } else {
            std::env::remove_var("POSTGRES_URL");
        }
    }
}

fn date(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 1, day, 12, 0, 0).unwrap()
}

fn request(class: AssetClass) -> CreateAssetRequest {
    CreateAssetRequest {
        name: "Private asset".into(),
        asset_class: class,
        currency: "EUR".into(),
        unit_label: if class == AssetClass::CashAccount { "EUR" } else { "units" }.into(),
        current_interest_rate_percent: (class == AssetClass::CashAccount).then_some(dec!(2)),
    }
}

#[tokio::test]
#[ignore = "Requires isolated TEST_POSTGRES_URL; run database suite single-threaded"]
async fn tenant_isolation_database_asset_cash_ledgers_and_interest() {
    let db = Database::new().await;
    let a = assets::create_asset(&db.owner, &request(AssetClass::CashAccount)).await.unwrap();
    let b = assets::create_asset(&db.other, &request(AssetClass::CashAccount)).await.unwrap();
    let opening = CreateAssetBalanceSnapshotRequest {
        date: date(1), kind: AssetBalanceSnapshotKind::Opening,
        balance: dec!(100), balance_eur: Some(dec!(100)), note: None,
    };
    let snapshot = assets::create_asset_balance_snapshot(&db.owner, &a.id, &opening).await.unwrap();
    let transaction = assets::create_asset_transaction(&db.owner, &a.id, &CreateAssetTransactionRequest {
        date: date(2), kind: AssetTransactionKind::Deposit, amount: dec!(20),
        amount_eur: Some(dec!(20)), currency: "EUR".into(), note: None,
    }).await.unwrap();
    let interest = assets::create_linked_interest(&db.owner, &a.id, &CreateLinkedInterestRequest {
        date: date(3), amount: dec!(5), amount_eur: Some(dec!(5)), currency: "EUR".into(),
        broker: None, principal: None, withholding_tax: None, withholding_tax_currency: None,
    }).await.unwrap();
    assets::create_asset_balance_snapshot(&db.other, &b.id, &CreateAssetBalanceSnapshotRequest {
        balance: dec!(250), balance_eur: Some(dec!(250)), ..opening.clone()
    }).await.unwrap();
    let revision = get_asset_activity_revision(&db.owner, &a.id).await.unwrap().unwrap();

    assert!(get_asset(&db.other, &a.id).await.unwrap().is_none());
    assert!(get_asset_detail(&db.other, &a.id).await.unwrap().is_none());
    assert!(get_asset_activity_revision(&db.other, &a.id).await.unwrap().is_none());
    assert!(list_asset_transactions(&db.other, &a.id).await.unwrap().is_empty());
    assert!(list_asset_balance_snapshots(&db.other, &a.id).await.unwrap().is_empty());
    assert!(list_linked_interest(&db.other, &a.id, Some(date(4))).await.unwrap().is_empty());
    assert!(get_interest_by_id(&db.other, &interest.id).await.unwrap().is_none());
    assert_eq!(list_assets(&db.owner, true).await.unwrap().len(), 1);
    assert_eq!(list_assets(&db.other, true).await.unwrap()[0].id, b.id);

    let metadata = UpdateAssetRequest {
        name: "Changed".into(), currency: "EUR".into(), unit_label: "EUR".into(),
        current_interest_rate_percent: Some(dec!(9)),
    };
    assert!(update_asset(&db.other, &a.id, &metadata).await.unwrap().is_none());
    assert!(archive_asset(&db.other, &a.id, revision).await.unwrap().is_none());
    assert!(restore_asset(&db.other, &a.id).await.unwrap().is_none());
    let missing = assets::update_asset(&db.other, &a.id, &metadata).await.unwrap_err();
    assert!(missing.downcast_ref::<assets::AssetNotFound>().is_some());
    assert!(assets::archive_asset(&db.other, &a.id).await.unwrap_err()
        .downcast_ref::<assets::AssetNotFound>().is_some());

    let update_transaction = UpdateAssetTransactionRequest {
        date: date(2), kind: AssetTransactionKind::Deposit, amount: dec!(999),
        amount_eur: Some(dec!(999)), currency: "EUR".into(), note: None,
    };
    assert!(assets::update_asset_transaction(&db.other, &b.id, &transaction.id, &update_transaction)
        .await.unwrap().is_none());
    assert!(!delete_asset_transaction(&db.other, &a.id, &transaction.id).await.unwrap());
    assert!(update_asset_balance_snapshot(&db.other, &b.id, &snapshot.id,
        &UpdateAssetBalanceSnapshotRequest {
            date: date(1), kind: AssetBalanceSnapshotKind::Opening, balance: dec!(999),
            balance_eur: Some(dec!(999)), note: None,
        }, dec!(999)).await.unwrap().is_none());
    assert!(!delete_asset_balance_snapshot(&db.other, &a.id, &snapshot.id).await.unwrap());
    assert!(!link_interest(&db.other, &b.id, &interest.id).await.unwrap());
    assert!(!unlink_interest(&db.other, &a.id, &interest.id).await.unwrap());
    assert!(db.client.execute(
        "INSERT INTO asset_transaction (user_id, asset_id, date, kind, amount, amount_eur, currency)
         VALUES ($1, $2, $3, 'Deposit', 1, 1, 'EUR')",
        &[&db.other.as_str(), &a.id, &date(4)],
    ).await.is_err());
    assert_eq!(get_asset_activity_revision(&db.owner, &a.id).await.unwrap(), Some(revision));

    let a_holdings = assets::calculate_custom_holdings_as_of(&db.owner, date(4)).await.unwrap();
    let b_holdings = assets::calculate_custom_holdings_as_of(&db.other, date(4)).await.unwrap();
    assert_eq!(a_holdings.len(), 1);
    assert_eq!(b_holdings.len(), 1);
    assert_eq!(a_holdings[0].units_or_balance, dec!(125));
    assert_eq!(b_holdings[0].units_or_balance, dec!(250));

    // Identical imported IDs resolve independently, and unlink preserves imports.
    for owner in [&db.owner, &db.other] {
        db.client.execute(
            "INSERT INTO interest (user_id, id, date, amount, amount_eur, principal, currency)
             VALUES ($1, 'same-import', $2, 1, 1, 'Cash', 'EUR')",
            &[&owner.as_str(), &date(4)],
        ).await.unwrap();
    }
    assert!(assets::link_interest(&db.owner, &a.id, "same-import").await.unwrap());
    assert!(assets::link_interest(&db.other, &b.id, "same-import").await.unwrap());
    assert!(assets::unlink_interest(&db.other, &b.id, "same-import").await.unwrap());
    assert!(get_interest_by_id(&db.other, "same-import").await.unwrap().unwrap().asset_id.is_none());
    assert_eq!(get_interest_by_id(&db.owner, "same-import").await.unwrap().unwrap().asset_id, Some(a.id));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "Requires isolated TEST_POSTGRES_URL; run database suite single-threaded"]
async fn tenant_isolation_database_asset_trade_valuation_and_archival() {
    let db = Database::new().await;
    let a = assets::create_asset(&db.owner, &request(AssetClass::RealEstate)).await.unwrap();
    let b = assets::create_asset(&db.other, &request(AssetClass::RealEstate)).await.unwrap();
    let buy = CreateAssetTradeRequest {
        date: date(1), units: dec!(2), price_per_unit: dec!(10), eur_price_per_unit: Some(dec!(10)),
        direction: AssetTradeDirection::Buy, currency: "EUR".into(), broker: "Test".into(),
        fees: dec!(1), note: None,
    };
    let trade = assets::create_asset_trade(&db.owner, &a.id, &buy).await.unwrap();
    let valuation = assets::create_asset_valuation(&db.owner, &a.id, &CreateAssetValuationRequest {
        date: date(2), price_per_unit: dec!(20), eur_price_per_unit: Some(dec!(20)), currency: "EUR".into(),
    }).await.unwrap();
    assert_eq!(list_asset_valuations(&db.owner, &a.id).await.unwrap().len(), 2);
    assert!(list_asset_trades(&db.other, &a.id).await.unwrap().is_empty());
    assert!(list_asset_valuations(&db.other, &a.id).await.unwrap().is_empty());
    let revision = get_asset_activity_revision(&db.owner, &a.id).await.unwrap().unwrap();
    let edit = UpdateAssetTradeRequest {
        date: date(1), units: dec!(4), price_per_unit: dec!(30), eur_price_per_unit: Some(dec!(30)),
        direction: AssetTradeDirection::Buy, currency: "EUR".into(), broker: "Test".into(),
        fees: dec!(2), note: None,
    };
    assert!(assets::update_asset_trade(&db.other, &b.id, &trade.id, &edit).await.unwrap().is_none());
    assert!(!delete_asset_trade(&db.other, &a.id, &trade.id).await.unwrap());
    let edit_valuation = UpdateAssetValuationRequest {
        date: date(2), price_per_unit: dec!(999), eur_price_per_unit: Some(dec!(999)), currency: "EUR".into(),
    };
    assert!(assets::update_asset_valuation(&db.other, &b.id, &valuation.id, &edit_valuation)
        .await.unwrap().is_none());
    assert!(!delete_asset_valuation(&db.other, &a.id, &valuation.id).await.unwrap());
    assert_eq!(get_asset_activity_revision(&db.owner, &a.id).await.unwrap(), Some(revision));
    assets::update_asset_trade(&db.owner, &a.id, &trade.id, &edit).await.unwrap().unwrap();
    let generated = list_asset_valuations(&db.owner, &a.id).await.unwrap()
        .into_iter().find(|row| row.id == trade.id).unwrap();
    assert_eq!(generated.price_per_unit, dec!(30));
    assert!(get_asset_activity_revision(&db.owner, &a.id).await.unwrap().unwrap() > revision);
    let revision = get_asset_activity_revision(&db.owner, &a.id).await.unwrap().unwrap();
    assert!(archive_asset(&db.owner, &a.id, revision - 1).await.unwrap().is_none());
    assert!(assets::archive_asset(&db.owner, &a.id).await.is_err());
    assert!(assets::delete_asset_trade(&db.owner, &a.id, &trade.id).await.unwrap());
    assert!(!list_asset_valuations(&db.owner, &a.id).await.unwrap().iter().any(|row| row.id == trade.id));
    assert!(assets::archive_asset(&db.owner, &a.id).await.unwrap().archived_at.is_some());
    assert!(list_assets(&db.owner, false).await.unwrap().is_empty());
    assert_eq!(list_assets(&db.owner, true).await.unwrap().len(), 1);
    assert_eq!(list_asset_valuations(&db.owner, &a.id).await.unwrap().len(), 1);
    assert!(assets::restore_asset(&db.owner, &a.id).await.unwrap().archived_at.is_none());
    db.cleanup().await;
}
