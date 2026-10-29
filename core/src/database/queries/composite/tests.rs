use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio_postgres::{Client, NoTls};

use super::get_monthly_investment_activity;

async fn test_database() -> (Client, String) {
    static NEXT_SCHEMA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Never fall back to POSTGRES_URL or a developer's .env database.
    let url = std::env::var("TEST_POSTGRES_URL")
        .expect("Set TEST_POSTGRES_URL to an isolated PostgreSQL instance");
    let (mut db, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    let schema = format!(
        "investment_activity_test_{}_{}_{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap(),
        NEXT_SCHEMA.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    db.batch_execute(&format!(
        "CREATE SCHEMA {schema}; SET search_path TO {schema}"
    ))
    .await
    .unwrap();
    refinery::embed_migrations!("migrations");
    migrations::runner().run_async(&mut db).await.unwrap();
    db.batch_execute(
        "INSERT INTO asset (id, name, asset_class, currency, unit_label, current_interest_rate_percent)
         VALUES ('gold', 'Gold', 'PhysicalGold', 'USD', 'oz', NULL),
                ('property', 'Property', 'RealEstate', 'EUR', 'units', NULL),
                ('cash', 'Savings', 'CashAccount', 'USD', 'USD', 2),
                ('debt', 'Loan', 'PrivateDebt', 'USD', 'USD', NULL)"
    )
    .await
    .unwrap();
    (db, schema)
}

async fn cleanup(db: &Client, schema: &str) {
    db.batch_execute(&format!(
        "SET search_path TO public; DROP SCHEMA {schema} CASCADE"
    ))
    .await
    .unwrap();
}

async fn assert_activity(db: &Client, expected: &[(&str, Decimal, Decimal, Decimal)]) {
    let activity = get_monthly_investment_activity(db, None).await.unwrap();
    let actual: Vec<_> = activity
        .iter()
        .map(|row| {
            (
                row.month.as_str(),
                row.net_purchases_eur,
                row.net_cash_added_eur,
                row.dividends_interest_eur,
            )
        })
        .collect();
    assert_eq!(actual.as_slice(), expected);
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn investment_activity_database_combines_ledgers_and_preserves_archived_history() {
    let (db, schema) = test_database().await;
    assert_activity(&db, &[]).await;
    db.batch_execute(
        "INSERT INTO trade (hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
                            security_type, direction, currency, isin, date_added, fees)
         VALUES ('buy', 'Test', '2025-01-02T12:00:00Z', 3, 100, 10,
                 'Equity', 'Buy', 'USD', 'test-isin', '2025-01-02T12:00:00Z', 99),
                ('sell', 'Test', '2025-01-03T12:00:00Z', 2, 40, 4,
                 'Equity', 'Sell', 'USD', 'test-isin', '2025-01-03T12:00:00Z', 99),
                ('april-sale', 'Test', '2025-04-01T12:00:00Z', 1, 90, 9,
                 'Equity', 'Sell', 'USD', 'test-isin', '2025-04-01T12:00:00Z', 99);
         INSERT INTO asset_trade (asset_id, date, units, price_per_unit, eur_price_per_unit,
                                  direction, currency, broker, fees, fees_eur)
         VALUES ('gold', '2025-01-02T12:00:00Z', 4, 50, 5, 'Buy', 'USD', 'Test', 990, 99),
                ('gold', '2025-01-03T12:00:00Z', 1, 20, 2, 'Sell', 'USD', 'Test', 990, 99),
                ('gold', '2025-03-01T12:00:00Z', 3, 250, 25, 'Sell', 'USD', 'Test', 990, 99),
                ('property', '2025-05-01T12:00:00Z', 3, 1000, 1000, 'Buy', 'EUR', 'Test', 99, 99);
         INSERT INTO asset_balance_snapshot (asset_id, date, kind, balance, balance_eur)
         VALUES ('cash', '2024-12-01T12:00:00Z', 'Opening', 100, 10);
         INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('cash', '2025-01-02T12:00:00Z', 'Deposit', 1000, 100, 'USD'),
                ('cash', '2025-01-03T12:00:00Z', 'Withdrawal', 300, 30, 'USD'),
                ('cash', '2025-02-01T12:00:00Z', 'Withdrawal', 800, 80, 'USD');
         INSERT INTO dividend (id, date, isin, amount, amount_eur, currency,
                               withholding_tax, withholding_tax_currency)
         VALUES ('dividend', '2025-01-04T12:00:00Z', 'test-isin', 110, 11, 'USD', 70, 'USD');
         INSERT INTO interest (id, asset_id, date, amount, amount_eur, principal, currency,
                               origin, tax_treatment, withholding_tax, withholding_tax_currency)
         VALUES ('unlinked-imported', NULL, '2025-01-05T12:00:00Z', 130, 13, 'Cash', 'USD',
                 'Imported', 'Included', 80, 'USD'),
                ('linked-imported', 'cash', '2025-01-06T12:00:00Z', 170, 17, 'Cash', 'USD',
                 'Imported', 'Included', 90, 'USD'),
                ('manual-cash', 'cash', '2025-01-07T12:00:00Z', 190, 19, 'Cash', 'USD',
                 'Manual', 'Excluded', NULL, NULL),
                ('manual-debt', 'debt', '2025-01-08T12:00:00Z', 230, 23, 'PrivateDebt', 'USD',
                 'Manual', 'Excluded', NULL, NULL);
         INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('cash', '2025-02-02T12:00:00Z', 'Withdrawal', 360, 36, 'USD');
         UPDATE asset SET archived_at = '2025-06-01T00:00:00Z' WHERE id IN ('gold', 'cash', 'debt')",
    )
    .await
    .unwrap();

    // Stored EUR amounts, not native currency, fees, or withholding, determine each series.
    // Linked imported interest is counted once; archived assets retain all recorded income.
    assert_activity(
        &db,
        &[
            ("2025-01", dec!(40), dec!(70), dec!(83)),
            ("2025-02", dec!(0), dec!(-116), dec!(0)),
            ("2025-03", dec!(-75), dec!(0), dec!(0)),
            ("2025-04", dec!(-9), dec!(0), dec!(0)),
            ("2025-05", dec!(3000), dec!(0), dec!(0)),
        ],
    )
    .await;
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn investment_activity_database_excludes_nonflow_activity_and_loan_principal() {
    let (db, schema) = test_database().await;
    db.batch_execute(
        "INSERT INTO asset_balance_snapshot (asset_id, date, kind, balance, balance_eur)
         VALUES ('cash', '2024-01-01T00:00:00Z', 'Opening', 10000, 1000),
                ('cash', '2024-02-01T00:00:00Z', 'Reconciliation', 11000, 1100);
         INSERT INTO asset_valuation (asset_id, date, price_per_unit, eur_price_per_unit, currency, source)
         VALUES ('gold', '2024-03-01T00:00:00Z', 20000, 2000, 'USD', 'Manual'),
                ('gold', '2024-04-01T00:00:00Z', 30000, 3000, 'USD', 'Trade');
         INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('debt', '2024-05-01T00:00:00Z', 'PrincipalAdvance', 1000, 100, 'USD'),
                ('debt', '2024-06-01T00:00:00Z', 'PrincipalRepayment', 250, 25, 'USD')",
    )
    .await
    .unwrap();

    // Excluded events must not even introduce otherwise empty months.
    assert_activity(&db, &[]).await;
    db.batch_execute(
        "INSERT INTO dividend (id, date, isin, amount, amount_eur, currency)
         VALUES ('income-only-dividend', '2024-07-02T00:00:00Z', 'test-isin', 100, 10, 'USD');
         INSERT INTO interest (id, asset_id, date, amount, amount_eur, principal, currency, origin, tax_treatment)
         VALUES ('imported-interest', NULL, '2024-07-01T00:00:00Z', 990, 99, 'Cash', 'USD', 'Imported', 'Included'),
                ('cash-interest', 'cash', '2024-08-01T00:00:00Z', 770, 77, 'Cash', 'USD', 'Manual', 'Excluded'),
                ('debt-interest', 'debt', '2024-09-01T00:00:00Z', 880, 88, 'PrivateDebt', 'USD', 'Manual', 'Excluded')",
    )
    .await
    .unwrap();
    // Recorded income introduces months even without purchases or cash-account flows.
    assert_activity(
        &db,
        &[
            ("2024-07", dec!(0), dec!(0), dec!(109)),
            ("2024-08", dec!(0), dec!(0), dec!(77)),
            ("2024-09", dec!(0), dec!(0), dec!(88)),
        ],
    )
    .await;
    db.batch_execute(
        "INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('cash', '2024-08-15T00:00:00Z', 'Deposit', 20, 2, 'USD')",
    )
    .await
    .unwrap();
    assert_activity(
        &db,
        &[
            ("2024-07", dec!(0), dec!(0), dec!(109)),
            ("2024-08", dec!(0), dec!(2), dec!(77)),
            ("2024-09", dec!(0), dec!(0), dec!(88)),
        ],
    )
    .await;
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn investment_activity_database_groups_every_ledger_by_utc_month() {
    let (db, schema) = test_database().await;
    db.batch_execute(
        "SET TIME ZONE 'Pacific/Honolulu';
         INSERT INTO trade (hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
                            security_type, direction, currency, isin, date_added)
         VALUES ('february', 'Test', '2025-03-01T00:30:00+01:00', 1, 1, 1,
                 'Equity', 'Buy', 'EUR', 'test-isin', '2025-01-01T00:00:00Z'),
                ('march', 'Test', '2025-02-28T23:30:00-01:00', 1, 2, 2,
                 'Equity', 'Buy', 'EUR', 'test-isin', '2025-01-01T00:00:00Z');
         INSERT INTO asset_trade (asset_id, date, units, price_per_unit, eur_price_per_unit,
                                  direction, currency, broker)
         VALUES ('gold', '2025-03-01T00:30:00+01:00', 1, 30, 3, 'Buy', 'USD', 'Test'),
                ('gold', '2025-02-28T23:30:00-01:00', 1, 40, 4, 'Buy', 'USD', 'Test');
         INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('cash', '2025-03-01T00:30:00+01:00', 'Deposit', 50, 5, 'USD'),
                ('cash', '2025-02-28T23:30:00-01:00', 'Deposit', 60, 6, 'USD');
         INSERT INTO dividend (id, date, isin, amount, amount_eur, currency)
         VALUES ('february-dividend', '2025-03-01T00:30:00+01:00', 'test-isin', 70, 7, 'USD'),
                ('march-dividend', '2025-02-28T23:30:00-01:00', 'test-isin', 80, 8, 'USD');
         INSERT INTO interest (id, date, amount, amount_eur, principal, currency)
         VALUES ('february-interest', '2025-03-01T00:30:00+01:00', 90, 9, 'Cash', 'USD'),
                ('march-interest', '2025-02-28T23:30:00-01:00', 100, 10, 'Cash', 'USD')",
    )
    .await
    .unwrap();

    let expected = [
        ("2025-02", dec!(4), dec!(5), dec!(16)),
        ("2025-03", dec!(6), dec!(6), dec!(18)),
    ];
    assert_activity(&db, &expected).await;
    db.batch_execute("SET TIME ZONE 'Pacific/Kiritimati'")
        .await
        .unwrap();
    assert_activity(&db, &expected).await;
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn investment_activity_database_filters_each_ledger_before_monthly_aggregation() {
    let (db, schema) = test_database().await;
    db.batch_execute(
        "SET TIME ZONE 'Pacific/Honolulu';
         INSERT INTO trade (hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
                            security_type, direction, currency, isin, date_added)
         VALUES ('before', 'Test', '2025-01-10T00:59:59+01:00', 1, 1000, 1000,
                 'Equity', 'Buy', 'EUR', 'test-isin', now()),
                ('at', 'Test', '2025-01-09T23:00:00-01:00', 1, 10, 10,
                 'Equity', 'Buy', 'EUR', 'test-isin', now());
         INSERT INTO asset_trade (asset_id, date, units, price_per_unit, eur_price_per_unit,
                                  direction, currency, broker)
         VALUES ('gold', '2025-01-09T23:59:59Z', 1, 1000, 1000, 'Buy', 'USD', 'Test'),
                ('gold', '2025-01-10T00:00:00Z', 1, 20, 20, 'Buy', 'USD', 'Test');
         INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('cash', '2025-01-09T23:59:59Z', 'Deposit', 1000, 1000, 'USD'),
                ('cash', '2025-01-10T00:00:00Z', 'Deposit', 30, 30, 'USD');
         INSERT INTO dividend (id, date, isin, amount, amount_eur, currency)
         VALUES ('before', '2025-01-09T23:59:59Z', 'test-isin', 1000, 1000, 'EUR'),
                ('at', '2025-01-10T00:00:00Z', 'test-isin', 40, 40, 'EUR');
         INSERT INTO interest (id, date, amount, amount_eur, principal, currency)
         VALUES ('old-month', '2024-12-31T23:59:59Z', 1000, 1000, 'Cash', 'EUR'),
                ('before', '2025-01-09T23:59:59Z', 1000, 1000, 'Cash', 'EUR'),
                ('at', '2025-01-10T00:00:00Z', 50, 50, 'Cash', 'EUR'),
                ('later', '2025-02-01T00:00:00Z', 60, 60, 'Cash', 'EUR')",
    )
    .await
    .unwrap();

    let start = "2025-01-10T00:00:00Z".parse().unwrap();
    let activity = get_monthly_investment_activity(&db, Some(start))
        .await
        .unwrap();
    let actual: Vec<_> = activity
        .iter()
        .map(|row| {
            (
                row.month.as_str(),
                row.net_purchases_eur,
                row.net_cash_added_eur,
                row.dividends_interest_eur,
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            ("2025-01", dec!(30), dec!(30), dec!(90)),
            ("2025-02", dec!(0), dec!(0), dec!(60)),
        ],
    );
    let after_activity = "2025-03-01T00:00:00Z".parse().unwrap();
    assert!(get_monthly_investment_activity(&db, Some(after_activity))
        .await
        .unwrap()
        .is_empty());
    cleanup(&db, &schema).await;
}

#[tokio::test]
#[ignore = "Requires an isolated TEST_POSTGRES_URL; also run in CI"]
async fn investment_activity_database_rounds_only_after_combining_ledgers() {
    let (db, schema) = test_database().await;
    db.batch_execute(
        "INSERT INTO trade (hash, broker, date, units, avg_price_per_unit, eur_avg_price_per_unit,
                            security_type, direction, currency, isin, date_added)
         VALUES ('buy', 'Test', '2025-01-01T00:00:00Z', 3, 0.01, 0.001,
                 'Equity', 'Buy', 'USD', 'test-isin', '2025-01-01T00:00:00Z'),
                ('sell', 'Test', '2025-02-01T00:00:00Z', 3, 0.01, 0.001,
                 'Equity', 'Sell', 'USD', 'test-isin', '2025-02-01T00:00:00Z');
         INSERT INTO asset_trade (asset_id, date, units, price_per_unit, eur_price_per_unit,
                                  direction, currency, broker)
         VALUES ('gold', '2025-01-01T00:00:00Z', 3, 0.01, 0.001, 'Buy', 'USD', 'Test'),
                ('gold', '2025-02-01T00:00:00Z', 3, 0.01, 0.001, 'Sell', 'USD', 'Test');
         INSERT INTO asset_transaction (asset_id, date, kind, amount, amount_eur, currency)
         VALUES ('cash', '2025-01-01T00:00:00Z', 'Deposit', 0.04, 0.004, 'USD'),
                ('cash', '2025-01-02T00:00:00Z', 'Deposit', 0.04, 0.004, 'USD'),
                ('cash', '2025-02-01T00:00:00Z', 'Withdrawal', 0.04, 0.004, 'USD'),
                ('cash', '2025-02-02T00:00:00Z', 'Withdrawal', 0.04, 0.004, 'USD');
         INSERT INTO dividend (id, date, isin, amount, amount_eur, currency)
         VALUES ('january-dividend', '2025-01-01T00:00:00Z', 'test-isin', 0.03, 0.003, 'USD'),
                ('february-dividend', '2025-02-01T00:00:00Z', 'test-isin', 0.04, 0.004, 'USD');
         INSERT INTO interest (id, date, amount, amount_eur, principal, currency)
         VALUES ('january-interest', '2025-01-01T00:00:00Z', 0.03, 0.003, 'Cash', 'USD'),
                ('february-interest', '2025-02-01T00:00:00Z', 0.04, 0.004, 'Cash', 'USD')",
    )
    .await
    .unwrap();

    // Each purchase ledger alone would round to zero; their combined +/-0.006 must not.
    // Dividend and interest amounts likewise round only after summing both income tables.
    assert_activity(
        &db,
        &[
            ("2025-01", dec!(0.01), dec!(0.01), dec!(0.01)),
            ("2025-02", dec!(-0.01), dec!(-0.01), dec!(0.01)),
        ],
    )
    .await;
    cleanup(&db, &schema).await;
}
