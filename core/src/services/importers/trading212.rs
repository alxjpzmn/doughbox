mod prepare;
mod reconcile;

pub(crate) use prepare::{
    canonical_decimal, canonical_timestamp, fingerprint_native, prepare_trading212_csv,
    PreparedT212Batch, PreparedT212Row, T212Financial,
};
pub(crate) use reconcile::{apply_trading212_batch, candidates_for_rows, create_event, evidence_conflicts, insert_financial, legacy_income_key_matches, withholding_reported, Candidate};

use crate::{
    database::{db_client, models::auth::UserId},
    services::trading::sync::{T212ImportContext, T212ImportOutcome},
};

/// All financial preparation (including external FX enrichment) precedes the
/// owner-wide ingestion transaction. API workers use these phases separately
/// so their session-lock client also owns all financial/checkpoint writes.
pub async fn extract_trading212_record(
    user_id: &UserId,
    file_content: &[u8],
    context: &T212ImportContext,
) -> anyhow::Result<T212ImportOutcome> {
    let prepared = prepare_trading212_csv(user_id, file_content).await?;
    apply_trading212_batch(user_id, &mut db_client().await?, prepared, context).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use chrono::Timelike;
    use serde_json::json;
    use std::collections::BTreeMap;
    use crate::services::parsers::parse_timestamp;

    const HEADERS: &[&str] = &[
        "Action", "Time", "ID", "ISIN", "No. of shares", "Price / share",
        "Currency (Price / share)", "Total", "Currency (Total)", "Exchange rate",
        "Currency conversion fee", "Currency (Currency conversion fee)",
        "Withholding tax", "Currency (Withholding tax)", "Notes",
        "Currency conversion from amount", "Currency (Currency conversion from amount)",
        "Currency conversion to amount", "Currency (Currency conversion to amount)",
    ];

    fn owner() -> UserId { UserId::from_database("t212-prepare-tests".to_owned()) }

    fn row(action: &str, id: &str, timestamp: &str) -> Vec<String> {
        [action, timestamp, id, "US0000000001", "2", "5", "EUR", "9", "EUR", "1",
            "0.25", "EUR", "1", "EUR", "", "", "", "", ""]
            .into_iter().map(str::to_owned).collect()
    }

    fn csv(headers: &[&str], rows: &[Vec<String>]) -> Vec<u8> {
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.write_record(headers).unwrap();
        for row in rows { writer.write_record(row).unwrap(); }
        writer.into_inner().unwrap()
    }

    #[test]
    fn canonical_identity_normalizes_decimal_time_and_ignores_evidence() {
        assert_eq!(canonical_decimal(dec!(-0.000)), "0");
        assert_eq!(canonical_decimal(dec!(12.3400)), "12.34");
        let mut payload = json!({
            "version": 1, "kind": "trade", "timestamp": "2025-01-01T01:00:00.123456789+01:00",
            "isin": "US0000000001", "direction": "Buy", "units": "2.00", "unit_price": "5.000",
            "currency": "USD", "fees": "1", "fees_currency": "USD",
        });
        let fingerprint = fingerprint_native(&payload).unwrap();
        let expected = json!(["t212-event-v1", "trade", "2025-01-01T00:00:00.123456Z",
            "US0000000001", "Buy", "2", "5", "USD"]);
        assert_eq!(fingerprint, blake3::hash(&serde_json::to_vec(&expected).unwrap()).to_hex().to_string());
        payload["timestamp"] = json!("2025-01-01T00:00:00.123456Z");
        payload["units"] = json!("2");
        payload["unit_price"] = json!("5");
        payload["fees"] = json!("99");
        assert_eq!(fingerprint, fingerprint_native(&payload).unwrap());
        payload["direction"] = json!("Sell");
        assert_ne!(fingerprint, fingerprint_native(&payload).unwrap());
        assert_eq!(canonical_timestamp(parse_timestamp("2025-01-01 00:00:00").unwrap()),
            "2025-01-01T00:00:00.000000Z");
    }

    #[test]
    fn currency_principal_and_unambiguous_fields_are_identity() {
        let mut payload = json!({"version": 1, "kind": "interest",
            "timestamp": "2025-01-01T00:00:00Z", "principal": "Cash", "amount": "1.00", "currency": "EUR"});
        let original = fingerprint_native(&payload).unwrap();
        payload["currency"] = json!("USD");
        assert_ne!(original, fingerprint_native(&payload).unwrap());
        payload["currency"] = json!("EUR");
        payload["principal"] = json!("Shares");
        assert_ne!(original, fingerprint_native(&payload).unwrap());
        payload["amount"] = json!("not a number");
        assert!(fingerprint_native(&payload).is_err());
        payload["version"] = json!(2);
        assert!(fingerprint_native(&payload).is_err());
    }

    #[tokio::test]
    async fn named_headers_empty_reports_and_skipped_actions() {
        let owner = owner();
        assert!(prepare_trading212_csv(&owner, b"").await.is_err());
        assert!(prepare_trading212_csv(&owner, b"Action,Time\n").await.is_err());
        for time_header in ["Time", "Time (UTC)"] {
            let mut headers = HEADERS.to_vec();
            headers[1] = time_header;
            let empty = prepare_trading212_csv(&owner, &csv(&headers, &[])).await.unwrap();
            assert!(empty.rows.is_empty());
            assert!(empty.skipped_actions.is_empty());
            let mut skipped = row("Deposit", "merchant information", "invalid timestamp");
            skipped[7] = "invalid amount".to_owned();
            let batch = prepare_trading212_csv(&owner, &csv(&headers, &[
                skipped.clone(), skipped, row("Withdrawal", "", ""), row("Card spending", "", ""),
            ])).await.unwrap();
            assert!(batch.rows.is_empty());
            assert_eq!(batch.skipped_actions, BTreeMap::from([
                ("Deposit".to_owned(), 2), ("Withdrawal".to_owned(), 1), ("Card spending".to_owned(), 1),
            ]));
        }
    }

    #[tokio::test]
    async fn gross_income_native_evidence_and_fee_conventions() {
        let timestamp = "2025-01-01 00:00:00.123456789";
        let mut fx = row("Currency conversion", "fx-ID", timestamp);
        fx[14] = "30 USD -> 27.40 EUR".to_owned();
        let batch = prepare_trading212_csv(&owner(), &csv(HEADERS, &[
            row("Market buy", "trade-ID", timestamp), row("Dividend (Ordinary)", "div-ID", timestamp),
            row("Interest on cash", "cash-ID", timestamp), row("Lending interest", "shares-ID", timestamp), fx,
        ])).await.unwrap();
        assert_eq!(batch.rows.len(), 5);
        for (index, row) in batch.rows.iter().enumerate() {
            assert_eq!(row.row_number, index as i32 + 1);
            assert!(row.raw_id.is_some());
            assert_eq!(row.native_payload["timestamp"], "2025-01-01T00:00:00.123456Z");
            assert_eq!(row.financial.date().nanosecond(), 123_456_000);
            assert_eq!(row.original_timestamp, timestamp);
        }
        match &batch.rows[0].financial {
            T212Financial::Trade(trade) => {
                assert_eq!(trade.avg_price_per_unit, dec!(5));
                assert_eq!(trade.eur_avg_price_per_unit, dec!(5));
                assert_eq!(trade.fees, dec!(0.25));
            }
            _ => panic!("expected trade"),
        }
        assert_eq!(batch.rows[0].native_payload["fees"], "0.25");
        match &batch.rows[1].financial {
            T212Financial::Dividend(dividend) => {
                assert_eq!(dividend.amount, dec!(10));
                assert_eq!(dividend.amount_eur, dec!(10));
                assert_eq!(dividend.withholding_tax, dec!(1));
            }
            _ => panic!("expected dividend"),
        }
        assert_eq!(batch.rows[1].native_payload["amount"], "10");
        assert_eq!(batch.rows[1].native_payload["withholding_tax"], "1");
        match &batch.rows[4].financial {
            T212Financial::Fx(fx) => {
                assert_eq!(fx.from_amount, dec!(30));
                assert_eq!(fx.to_amount, dec!(27.4));
                assert_eq!(fx.fees, dec!(-0.25));
            }
            _ => panic!("expected FX"),
        }
    }

    #[tokio::test]
    async fn occurrence_is_timestamp_local_stable_and_preserves_opaque_ids() {
        let time = "2025-01-01 00:00:00";
        let batch = prepare_trading212_csv(&owner(), &csv(HEADERS, &[
            row("Interest on cash", "z", time), row("Interest on cash", "  ", time),
            row("Interest on cash", " A ", time), row("Interest on cash", "z", time),
            row("Interest on cash", "next", "2025-01-01 00:00:01"),
        ])).await.unwrap();
        assert_eq!(batch.rows.iter().map(|row| row.occurrence).collect::<Vec<_>>(), vec![3, 1, 2, 4, 1]);
        assert_eq!(batch.rows[1].raw_id, None);
        assert_eq!(batch.rows[2].raw_id.as_deref(), Some(" A "));
        assert_eq!(batch.rows[0].fingerprint, batch.rows[3].fingerprint);
        assert_ne!(batch.rows[0].row_number, batch.rows[3].row_number);
    }

    #[tokio::test]
    async fn malformed_supported_rows_and_invalid_csv_fail_closed() {
        let time = "2025-01-01 00:00:00";
        for (action, column, value) in [
            ("Market buy", 4, ""), ("Market buy", 5, "invalid"), ("Market buy", 9, "0"),
            ("Market buy", 6, ""), ("Dividend", 4, ""), ("Dividend", 5, ""),
            ("Interest on cash", 7, ""), ("Lending interest", 8, "Unknown"),
            ("Interest on cash", 1, "not a time"),
        ] {
            let mut malformed = row(action, "bad", time);
            malformed[column] = value.to_owned();
            // The first row would need ECB enrichment if validation were interleaved.
            let mut valid = row("Interest on cash", "valid", time);
            valid[8] = "USD".to_owned();
            assert!(prepare_trading212_csv(&owner(), &csv(HEADERS, &[valid, malformed])).await.is_err());
        }
        let mut invalid = csv(HEADERS, &[]);
        invalid.extend_from_slice(b"\"unterminated");
        assert!(prepare_trading212_csv(&owner(), &invalid).await.is_err());
        let mut invalid = csv(HEADERS, &[]);
        invalid.extend_from_slice(b"only,two\n");
        assert!(prepare_trading212_csv(&owner(), &invalid).await.is_err());
    }

    #[tokio::test]
    async fn fx_named_fields_and_notes_require_both_positive_sides() {
        let mut named = row("Currency conversion", "opaque-FX", "2025-01-01 00:00:00");
        named[15] = "30.00".to_owned();
        named[16] = "USD".to_owned();
        named[17] = "27.40".to_owned();
        named[18] = "EUR".to_owned();
        named[14] = "unrelated free text".to_owned();
        let batch = prepare_trading212_csv(&owner(), &csv(HEADERS, &[named.clone()])).await.unwrap();
        assert_eq!(batch.rows[0].native_payload["from_amount"], "30");
        assert_eq!(batch.rows[0].native_payload["to_amount"], "27.4");
        for (column, value) in [(15, "0"), (17, "-1"), (17, ""), (18, "Unknown"), (18, "")] {
            let mut invalid = named.clone();
            invalid[column] = value.to_owned();
            assert!(prepare_trading212_csv(&owner(), &csv(HEADERS, &[invalid])).await.is_err());
        }
        for notes in ["30 USD", "30 USD -> 0 EUR", "30 USD -> 27 Unknown", "USD -> EUR"] {
            let mut invalid = row("Currency conversion", "", "2025-01-01 00:00:00");
            invalid[14] = notes.to_owned();
            assert!(prepare_trading212_csv(&owner(), &csv(HEADERS, &[invalid])).await.is_err());
        }
    }
}
