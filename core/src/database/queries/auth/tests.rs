use chrono::Utc;
use rust_decimal::Decimal;

use crate::{
    database::{models::auth::UserId, queries::composite},
    services::auth::tests::{cleanup, test_database_at},
};

#[tokio::test]
#[ignore = "Requires a dedicated TEST_POSTGRES_URL; also run in CI"]
async fn tenant_isolation_database_owner_constraints_and_aggregate_boundaries() {
    let (db, schema) = test_database_at(refinery::Target::Latest).await;
    let a = UserId::new();
    let b = UserId::new();
    for (id, username) in [(&a, "alice"), (&b, "bob")] {
        db.execute("INSERT INTO auth_users(id,username,password_hash,role,status,totp_secret_encrypted) VALUES($1,$2,'fixture-phc','member','active','fixture-encrypted')", &[&id.as_str(),&username]).await.unwrap();
        db.execute("INSERT INTO asset(user_id,id,name,asset_class,currency,unit_label,current_interest_rate_percent) VALUES($1,$2,'Savings','CashAccount','EUR','EUR',0)", &[&id.as_str(),&format!("cash-{}",username)]).await.unwrap();
    }
    db.execute("INSERT INTO asset_balance_snapshot(user_id,asset_id,date,kind,balance,balance_eur) VALUES($1,'cash-alice','2026-01-01Z','Opening',100,100)", &[&a.as_str()]).await.unwrap();
    db.execute("INSERT INTO asset_balance_snapshot(user_id,asset_id,date,kind,balance,balance_eur) VALUES($1,'cash-bob','2026-01-01Z','Opening',250,250)", &[&b.as_str()]).await.unwrap();
    let cross=db.execute("INSERT INTO asset_transaction(user_id,asset_id,date,kind,amount,amount_eur,currency) VALUES($1,'cash-bob','2026-01-02Z','Deposit',10,10,'EUR')", &[&a.as_str()]).await.unwrap_err();
    assert_eq!(
        cross.code(),
        Some(&tokio_postgres::error::SqlState::FOREIGN_KEY_VIOLATION)
    );
    assert!(db
        .execute(
            "UPDATE asset SET user_id=$1 WHERE user_id=$2",
            &[&b.as_str(), &a.as_str()]
        )
        .await
        .is_err());
    let date = Utc::now();
    for (owner, amount) in [(&a, Decimal::from(10)), (&b, Decimal::from(30))] {
        db.execute("INSERT INTO dividend(user_id,id,date,isin,amount,amount_eur,currency) VALUES($1,'identical-statement-id',$2,'fixture-isin',$3,$3,'EUR') ON CONFLICT(user_id,id) DO NOTHING", &[&owner.as_str(),&date,&amount]).await.unwrap();
    }
    db.execute("INSERT INTO dividend(user_id,id,date,isin,amount,amount_eur,currency) VALUES($1,'identical-statement-id',$2,'fixture-isin',999,999,'EUR') ON CONFLICT(user_id,id) DO NOTHING", &[&a.as_str(),&date]).await.unwrap();
    let a_rows = composite::get_monthly_investment_activity(&a, &db, None)
        .await
        .unwrap();
    let b_rows = composite::get_monthly_investment_activity(&b, &db, None)
        .await
        .unwrap();
    assert_eq!(
        a_rows
            .iter()
            .map(|v| v.dividends_interest_eur)
            .sum::<Decimal>(),
        Decimal::from(10)
    );
    assert_eq!(
        b_rows
            .iter()
            .map(|v| v.dividends_interest_eur)
            .sum::<Decimal>(),
        Decimal::from(30)
    );
    db.execute("INSERT INTO interest(user_id,id,asset_id,date,amount,amount_eur,principal,currency) VALUES($1,'linked-interest','cash-alice','2026-01-02Z',1,1,'Cash','EUR')", &[&a.as_str()]).await.unwrap();
    db.execute(
        "DELETE FROM asset WHERE user_id=$1 AND id='cash-alice'",
        &[&a.as_str()],
    )
    .await
    .unwrap();
    let row = db
        .query_one(
            "SELECT user_id,asset_id FROM interest WHERE user_id=$1 AND id='linked-interest'",
            &[&a.as_str()],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), a.as_str());
    assert!(row.get::<_, Option<String>>(1).is_none());
    assert_eq!(
        db.query_one(
            "SELECT balance FROM asset_balance_snapshot WHERE user_id=$1",
            &[&b.as_str()]
        )
        .await
        .unwrap()
        .get::<_, Decimal>(0),
        Decimal::from(250)
    );
    cleanup(db, &schema).await;
}
