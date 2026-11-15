//! Owner-local reconciliation of staged financial observations. Review never
//! contacts the broker or recalculates the financial values saved by preparation.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::ensure;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use tokio_postgres::Row;
use utoipa::ToSchema;

use super::{store, T212ImportBatchSummary, T212SyncError, T212SyncStatus};
use crate::{
    database::{db_client, models::auth::UserId, queries::trading212_import},
    services::{
        auth::AuthError,
        importers::trading212::{
            candidates_for_rows, canonical_timestamp, create_event, fingerprint_native,
            insert_financial, legacy_income_key_matches, Candidate, PreparedT212Row, T212Financial,
        },
        shared::util::hash_string,
    },
};

#[derive(Debug, Serialize, ToSchema)]
pub struct T212ReviewCandidate {
    pub kind: String,
    pub ledger_id: String,
    pub event_id: Option<String>,
    pub account_id: Option<String>,
    pub import_source: Option<String>,
    pub imported_at: Option<DateTime<Utc>>,
    pub native: Value,
    pub recorded: Value,
    pub values_differ: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct T212ReviewItem {
    pub batch_id: String,
    pub row_number: i32,
    pub version: i32,
    pub source: String,
    pub account_id: Option<String>,
    pub raw_id: Option<String>,
    pub raw_action: String,
    pub kind: String,
    pub native: Value,
    pub normalized: Value,
    pub reason: String,
    pub candidates: Vec<T212ReviewCandidate>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct T212ReviewPage {
    pub items: Vec<T212ReviewItem>,
    pub next_cursor: Option<i32>,
}

// Unlike an ordinary Option field, this has no implicit missing-field default.
// Null is an explicit account choice; an omitted account is not a choice.
fn required_account<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum T212ReviewDecision {
    MatchExisting {
        batch_id: String,
        row_number: i32,
        version: i32,
        #[serde(deserialize_with = "required_account")]
        #[schema(required = true, nullable = true)]
        account_id: Option<String>,
        kind: String,
        ledger_id: String,
        accept_recorded_values: bool,
    },
    ImportNew {
        batch_id: String,
        row_number: i32,
        version: i32,
        #[serde(deserialize_with = "required_account")]
        #[schema(required = true, nullable = true)]
        account_id: Option<String>,
    },
}

impl T212ReviewDecision {
    fn location(&self) -> (&str, i32, i32, &Option<String>) {
        match self {
            Self::MatchExisting { batch_id, row_number, version, account_id, .. }
            | Self::ImportNew { batch_id, row_number, version, account_id } => {
                (batch_id, *row_number, *version, account_id)
            }
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct T212ReviewRequest {
    pub decisions: Vec<T212ReviewDecision>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct T212ReviewResponse {
    pub batches: Vec<T212ImportBatchSummary>,
    pub status: T212SyncStatus,
}

fn invalid() -> anyhow::Error {
    T212SyncError {
        code: "TradingSyncInvalidReview",
        message: "Choose 1–200 valid review decisions with one explicit account choice per batch.",
    }.into()
}

fn changed() -> anyhow::Error {
    T212SyncError {
        code: "TradingSyncReviewChanged",
        message: "This review or its recorded activity has changed. Refresh the review before deciding again.",
    }.into()
}

fn safe_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("Recorded fee or withholding evidence differs") => "Recorded fee or withholding evidence differs",
        Some("A previously observed broker ID has changed native economics") => "A previously observed broker ID has changed native economics",
        Some("Equal native economics have disjoint broker IDs; confirm whether these are the same transaction") => "Equal native economics have disjoint broker IDs; confirm whether these are the same transaction",
        Some("Statement does not establish complete timestamp-bucket cardinality") => "Statement does not establish complete timestamp-bucket cardinality",
        Some("Legacy trade ID conflicts with native economics or another account") => "Legacy trade ID conflicts with native economics or another account",
        Some("Legacy order ID was shared by multiple execution occurrences") => "Legacy order ID was shared by multiple execution occurrences",
        Some("Assigning this formerly unassigned file would collide with known account activity") => "Assigning this formerly unassigned file would collide with known account activity",
        _ => "Historical account ownership or occurrence association requires confirmation",
    }
}

fn prepared_row(row: &Row) -> anyhow::Result<PreparedT212Row> {
    let normalized: Value = row.get("normalized_payload");
    let native: Value = row.get("native_payload");
    ensure!(normalized.get("version").and_then(Value::as_u64) == Some(1), changed());
    let financial: T212Financial = serde_json::from_value(
        normalized.get("event").cloned().ok_or_else(changed)?,
    ).map_err(|_| changed())?;
    let fingerprint: String = row.get("fingerprint");
    ensure!(
        fingerprint_native(&native).map_err(|_| changed())? == fingerprint
            && row.get::<_, String>("kind") == financial.kind()
            && native.get("kind").and_then(Value::as_str) == Some(financial.kind())
            && native.get("timestamp").and_then(Value::as_str)
                == Some(canonical_timestamp(financial.date()).as_str()),
        changed()
    );
    Ok(PreparedT212Row {
        row_number: row.get("row_number"),
        raw_action: row.get("raw_action"),
        raw_id: row.get("raw_id"),
        original_timestamp: row.get("original_timestamp"),
        fingerprint,
        occurrence: row.get("occurrence"),
        native_payload: native,
        financial,
    })
}

fn legacy_trade_id(row: &PreparedT212Row, candidate: &Candidate) -> bool {
    candidate.kind == "trade" && row.financial.kind() == "trade"
        && row.raw_id.as_ref().is_some_and(|id| {
            candidate.ledger_id == hash_string(&format!("Trading212{id}"))
        })
}

fn evidence_backed(row: &PreparedT212Row, candidate: &Candidate) -> bool {
    if row.financial.kind() != candidate.kind {
        return false;
    }
    if candidate.matches_fingerprint(&row.fingerprint) || legacy_trade_id(row, candidate) || legacy_income_key_matches(row, candidate) {
        return true;
    }
    // Review, unlike automatic reconciliation, permits a correction to be
    // linked by an exact observed ID. Shared IDs may suggest several fills:
    // explicit selection, one-to-one use and the value acknowledgment are
    // separately enforced by resolve.
    row.raw_id.as_ref().is_some_and(|raw_id| candidate.has_raw_id(raw_id))
}

fn tax_equal(a: Decimal, a_currency: &str, b: Decimal, b_currency: &str) -> bool {
    a == b && (a.is_zero() || a_currency == b_currency)
}

fn financial_equal(a: &T212Financial, b: &T212Financial) -> bool {
    match (a, b) {
        (T212Financial::Trade(a), T212Financial::Trade(b)) => {
            a.date == b.date && a.isin == b.isin && a.direction == b.direction
                && a.units == b.units && a.avg_price_per_unit == b.avg_price_per_unit
                && a.currency == b.currency && a.eur_avg_price_per_unit == b.eur_avg_price_per_unit
                && a.security_type == b.security_type && a.fees == b.fees
                && tax_equal(a.withholding_tax, &a.withholding_tax_currency, b.withholding_tax, &b.withholding_tax_currency)
        }
        (T212Financial::Dividend(a), T212Financial::Dividend(b)) => {
            a.date == b.date && a.isin == b.isin && a.amount == b.amount
                && a.currency == b.currency && a.amount_eur == b.amount_eur
                && tax_equal(a.withholding_tax, &a.withholding_tax_currency, b.withholding_tax, &b.withholding_tax_currency)
        }
        (T212Financial::Interest(a), T212Financial::Interest(b)) => {
            a.date == b.date && a.principal == b.principal && a.amount == b.amount
                && a.currency == b.currency && a.amount_eur == b.amount_eur
                && tax_equal(a.withholding_tax, &a.withholding_tax_currency, b.withholding_tax, &b.withholding_tax_currency)
        }
        (T212Financial::Fx(a), T212Financial::Fx(b)) => {
            a.date == b.date && a.from_amount == b.from_amount && a.to_amount == b.to_amount
                && a.from_currency == b.from_currency && a.to_currency == b.to_currency && a.fees == b.fees
        }
        _ => false,
    }
}

fn native_evidence_equal(a: &Value, b: &Value) -> bool {
    [("fees", "fees_currency"), ("withholding_tax", "withholding_tax_currency")].into_iter().all(|(key, currency)| {
        let amount = |value: &Value| -> Option<Decimal> {
            match value.get(key) {
                None => Some(Decimal::ZERO),
                Some(value) => value.as_str()?.parse().ok(),
            }
        };
        match (amount(a), amount(b)) {
            (Some(a_amount), Some(b_amount)) if a_amount == b_amount => {
                a_amount.is_zero() || a.get(currency) == b.get(currency)
            }
            _ => false,
        }
    })
}

fn values_differ(row: &PreparedT212Row, candidate: &Candidate) -> bool {
    !financial_equal(&row.financial, &candidate.financial)
        || (!candidate.observations.is_empty() && !candidate.observations.iter()
            .any(|(_, native)| native_evidence_equal(&row.native_payload, native)))
}

/// SQL-only, owner-scoped pagination over unresolved rows, including CLI batches
/// whose source upload job no longer exists. Cursor is the last row ordinal.
pub async fn list(
    user_id: &UserId, batch_id: &str, cursor: Option<i32>, limit: usize,
) -> anyhow::Result<T212ReviewPage> {
    ensure!(cursor.is_none_or(|cursor| cursor >= 0) && (1..=200).contains(&limit), invalid());
    let mut db = db_client().await?;
    let tx = db.transaction().await?;
    let batch = tx.query_opt(
        "SELECT source,account_id FROM t212_import_batch WHERE user_id=$1 AND id=$2",
        &[&user_id.as_str(), &batch_id],
    ).await?.ok_or(AuthError::NotFound)?;
    let mut rows = tx.query(
        "SELECT * FROM t212_import_row WHERE user_id=$1 AND batch_id=$2
         AND state IN ('pending','review') AND row_number>$3 ORDER BY row_number LIMIT $4",
        &[&user_id.as_str(), &batch_id, &cursor.unwrap_or(0), &(limit as i64 + 1)],
    ).await?;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    if rows.is_empty() {
        tx.commit().await?;
        return Ok(T212ReviewPage { items: Vec::new(), next_cursor: None });
    }
    let incoming = rows.iter().map(prepared_row).collect::<anyhow::Result<Vec<_>>>()?;
    let candidates = candidates_for_rows(user_id, &tx, &incoming).await?;
    let event_ids: Vec<String> = candidates.iter().filter_map(|candidate| candidate.event_id.clone()).collect();
    let provenance: HashMap<String, (String, Option<DateTime<Utc>>)> = tx.query(
        "SELECT id,created_via,imported_at FROM t212_activity_event WHERE user_id=$1 AND id=ANY($2)",
        &[&user_id.as_str(), &event_ids],
    ).await?.into_iter().map(|row| (row.get("id"), (row.get("created_via"), row.get("imported_at")))).collect();
    let mut items = Vec::with_capacity(rows.len());
    for (stored, row) in rows.iter().zip(&incoming) {
        let mut suggested = Vec::new();
        for candidate in candidates.iter().filter(|candidate| evidence_backed(row, candidate)) {
            let metadata = candidate.event_id.as_ref().and_then(|id| provenance.get(id));
            let mut native = candidate.native();
            let mut raw_ids: Vec<&str> = candidate.observations.iter().filter_map(|(id,_)|id.as_deref()).collect();
            raw_ids.sort_unstable();
            raw_ids.dedup();
            native["raw_ids"] = json!(raw_ids);
            suggested.push(T212ReviewCandidate {
                kind: candidate.kind.clone(),
                ledger_id: candidate.ledger_id.clone(),
                event_id: candidate.event_id.clone(),
                account_id: candidate.account_id.clone(),
                import_source: metadata.map(|(source, _)| source.clone()),
                imported_at: metadata.and_then(|(_, at)| *at),
                native,
                recorded: serde_json::to_value(&candidate.financial)?,
                values_differ: values_differ(row, candidate),
            });
        }
        items.push(T212ReviewItem {
            batch_id: batch_id.to_owned(),
            row_number: row.row_number,
            version: stored.get("version"),
            source: batch.get("source"),
            account_id: batch.get("account_id"),
            raw_id: row.raw_id.clone(),
            raw_action: row.raw_action.clone(),
            kind: row.financial.kind().into(),
            native: row.native_payload.clone(),
            normalized: stored.get("normalized_payload"),
            reason: safe_reason(stored.get::<_, Option<String>>("review_reason").as_deref()).into(),
            candidates: suggested,
        });
    }
    let next_cursor = has_more.then(|| items.last().expect("nonempty full page").row_number);
    tx.commit().await?;
    Ok(T212ReviewPage { items, next_cursor })
}

// The concurrency version is a precondition, not part of the financial choice.
// Preserve the exact accepted request in storage, but allow an identical choice
// to be replayed with either the original or refreshed version.
fn same_resolution(saved: &Value, incoming: &Value) -> bool {
    match (saved.as_object(), incoming.as_object()) {
        (Some(saved), Some(incoming)) => {
            saved.len() - usize::from(saved.contains_key("version"))
                == incoming.len() - usize::from(incoming.contains_key("version"))
                && saved.iter().filter(|(key, _)| key.as_str() != "version")
                    .all(|(key, value)| incoming.get(key) == Some(value))
        }
        _ => false,
    }
}

fn ledger_location(kind: &str) -> anyhow::Result<(&'static str, &'static str, &'static str)> {
    match kind {
        "trade" => Ok(("trade", "hash", "trade_hash")),
        "dividend" => Ok(("dividend", "id", "dividend_id")),
        "interest" => Ok(("interest", "id", "interest_id")),
        "fx" => Ok(("fx_conversion", "id", "fx_conversion_id")),
        _ => Err(invalid()),
    }
}

/// Apply explicit decisions atomically with the same owner-wide ingestion lock
/// as CSV/API ingestion. Credentials are not needed to resolve staged data;
/// finish_batch independently fences any attempt to advance API coverage.
pub async fn resolve(
    user_id: &UserId, decisions: Vec<T212ReviewDecision>,
) -> anyhow::Result<T212ReviewResponse> {
    ensure!((1..=200).contains(&decisions.len()), invalid());
    let mut accounts = BTreeMap::<String, Option<String>>::new();
    let mut requested = BTreeMap::<(String, i32), T212ReviewDecision>::new();
    let mut matches = BTreeMap::<(String, String), ()>::new();
    for decision in decisions {
        let (batch, ordinal, version, account) = decision.location();
        ensure!(!batch.is_empty() && ordinal > 0 && version > 0, invalid());
        if let Some(previous) = accounts.get(batch) {
            ensure!(previous == account, invalid());
        } else {
            accounts.insert(batch.to_owned(), account.clone());
        }
        if let T212ReviewDecision::MatchExisting { kind, ledger_id, .. } = &decision {
            ledger_location(kind)?;
            ensure!(!ledger_id.is_empty(), invalid());
            matches.insert((kind.clone(), ledger_id.clone()), ());
        }
        let key = (batch.to_owned(), ordinal);
        if let Some(previous) = requested.get(&key) {
            ensure!(same_resolution(&serde_json::to_value(previous)?, &serde_json::to_value(&decision)?), changed());
        } else {
            requested.insert(key, decision);
        }
    }

    let mut db = db_client().await?;
    let tx = db.transaction().await?;
    trading212_import::lock_owner(user_id, &tx).await?;
    let batch_ids: Vec<String> = accounts.keys().cloned().collect();
    // Discover whether the connection lock is needed before taking ingestion
    // ownership. Owner-first serialization makes this discovery stable.
    let discovered = tx.query(
        "SELECT id,source FROM t212_import_batch WHERE user_id=$1 AND id=ANY($2) ORDER BY id",
        &[&user_id.as_str(), &batch_ids],
    ).await?;
    ensure!(discovered.len() == batch_ids.len(), AuthError::NotFound);
    if discovered.iter().any(|row| row.get::<_, String>("source") == "trading212_api") {
        tx.query_opt("SELECT user_id FROM trading_connection WHERE user_id=$1 FOR UPDATE", &[&user_id.as_str()]).await?;
    }
    trading212_import::lock_ingestion(user_id, &tx).await?;
    let batches = tx.query(
        "SELECT * FROM t212_import_batch WHERE user_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE",
        &[&user_id.as_str(), &batch_ids],
    ).await?;
    let mut account_ids: Vec<String> = accounts.values().filter_map(Clone::clone).collect();
    account_ids.sort_unstable();
    account_ids.dedup();
    let owned_accounts = tx.query(
        "SELECT id FROM t212_sync_account WHERE user_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE",
        &[&user_id.as_str(), &account_ids],
    ).await?;
    ensure!(owned_accounts.len() == account_ids.len(), AuthError::NotFound);
    for batch in &batches {
        let id: String = batch.get("id");
        let previous: Option<String> = batch.get("account_id");
        let desired = &accounts[&id];
        ensure!(previous.is_none() || &previous == desired, changed());
        ensure!(batch.get::<_, String>("source") != "trading212_api" || previous.as_ref() == desired.as_ref(), changed());
        let chosen = serde_json::to_value(desired)?;
        let conflicting = tx.query_opt(
            "SELECT 1 FROM t212_import_row WHERE user_id=$1 AND batch_id=$2
             AND resolution IS NOT NULL AND resolution->'account_id' IS DISTINCT FROM $3::jsonb LIMIT 1",
            &[&user_id.as_str(), &id, &chosen],
        ).await?;
        ensure!(conflicting.is_none(), changed());
    }

    let mut stored_rows = BTreeMap::new();
    // Includes automatic matches/insertions, not merely decisions in this request.
    let mut used = HashMap::<(String, String), i32>::new();
    let mut linked_events = HashSet::<String>::new();
    for batch_id in &batch_ids {
        let ordinals: Vec<i32> = requested.keys().filter_map(|(batch, ordinal)| (batch == batch_id).then_some(*ordinal)).collect();
        for row in tx.query(
            "SELECT * FROM t212_import_row WHERE user_id=$1 AND batch_id=$2 AND row_number=ANY($3) ORDER BY row_number FOR UPDATE",
            &[&user_id.as_str(), batch_id, &ordinals],
        ).await? {
            stored_rows.insert((batch_id.clone(), row.get::<_, i32>("row_number")), row);
        }
        for row in tx.query(
            "SELECT o.row_number,e.id,e.kind,COALESCE(e.trade_hash,e.dividend_id,e.interest_id,e.fx_conversion_id) AS ledger_id
             FROM t212_import_row o JOIN t212_activity_event e ON e.user_id=o.user_id AND e.id=o.event_id
             WHERE o.user_id=$1 AND o.batch_id=$2 ORDER BY o.row_number FOR UPDATE OF o",
            &[&user_id.as_str(), batch_id],
        ).await? {
            let identity = format!("{}:{}", row.get::<_, String>("kind"), row.get::<_, String>("ledger_id"));
            let ordinal: i32 = row.get("row_number");
            if let Some(previous) = used.insert((batch_id.clone(), identity), ordinal) {
                ensure!(previous == ordinal, changed());
            }
            linked_events.insert(row.get("id"));
        }
    }
    // A missing row in an owned batch is stale review state, not a foreign batch.
    ensure!(stored_rows.len() == requested.len(), changed());

    // Lock retained ledger identities before reading their financial values.
    // SQL identifiers come only from the closed server-side kind map.
    for (kind, ledger_id) in matches.keys() {
        let (table, key, link) = ledger_location(kind)?;
        let sql = format!("SELECT {key} FROM {table} WHERE user_id=$1 AND {key}=$2 AND broker='Trading212' FOR UPDATE");
        ensure!(tx.query_opt(&sql, &[&user_id.as_str(), ledger_id]).await?.is_some(), changed());
        let sql = format!("SELECT id FROM t212_activity_event WHERE user_id=$1 AND {link}=$2");
        if let Some(row) = tx.query_opt(&sql, &[&user_id.as_str(), ledger_id]).await? {
            linked_events.insert(row.get("id"));
        }
    }
    let mut event_ids: Vec<String> = linked_events.into_iter().collect();
    event_ids.sort_unstable();
    let locked_events = tx.query(
        "SELECT id,account_id FROM t212_activity_event WHERE user_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE",
        &[&user_id.as_str(), &event_ids],
    ).await?;
    let event_accounts: HashMap<String, Option<String>> = locked_events.into_iter().map(|row| (row.get("id"), row.get("account_id"))).collect();
    let incoming = stored_rows.values().map(prepared_row).collect::<anyhow::Result<Vec<_>>>()?;
    let mut candidates = candidates_for_rows(user_id, &tx, &incoming).await?;
    let sources: HashMap<String, String> = batches.iter().map(|batch| (batch.get("id"), batch.get("source"))).collect();

    for (((batch_id, ordinal), decision), row) in requested.iter().zip(&incoming) {
        let stored = &stored_rows[&(batch_id.clone(), *ordinal)];
        let (_, _, version, account) = decision.location();
        let canonical = serde_json::to_value(decision)?;
        if let Some(saved) = stored.get::<_, Option<Value>>("resolution") {
            ensure!(same_resolution(&saved, &canonical), changed());
            let event_id: Option<String> = stored.get("event_id");
            let state: String = stored.get("state");
            ensure!(matches!(state.as_str(), "matched" | "inserted")
                && event_id.as_ref().is_some_and(|id| event_accounts.contains_key(id)), changed());
            continue;
        }
        ensure!(stored.get::<_, i32>("version") == version
            && matches!(stored.get::<_, String>("state").as_str(), "pending" | "review"), changed());
        let (event_id, state) = match decision {
            T212ReviewDecision::MatchExisting { kind, ledger_id, accept_recorded_values, .. } => {
                let candidate = candidates.iter_mut().find(|candidate| &candidate.kind == kind && &candidate.ledger_id == ledger_id).ok_or_else(changed)?;
                ensure!(evidence_backed(row, candidate), changed());
                ensure!(!values_differ(row, candidate) || *accept_recorded_values, invalid());
                ensure!(candidate.account_id.is_none() || &candidate.account_id == account, changed());
                let identity = candidate.identity();
                if let Some(previous) = used.get(&(batch_id.clone(), identity.clone())) {
                    ensure!(*previous == *ordinal, changed());
                }
                used.insert((batch_id.clone(), identity), *ordinal);
                let id = if let Some(id) = &candidate.event_id {
                    id.clone()
                } else {
                    let id = uuid::Uuid::new_v4().to_string();
                    create_event(user_id, &tx, &id, account.as_deref(), kind, &candidate.fingerprint, "legacy", ledger_id).await?;
                    id
                };
                if account.is_some() {
                    tx.execute("UPDATE t212_activity_event SET account_id=$3 WHERE user_id=$1 AND id=$2 AND account_id IS NULL", &[&user_id.as_str(), &id, account]).await?;
                }
                candidate.event_id = Some(id.clone());
                if account.is_some() {
                    candidate.account_id = account.clone();
                }
                (id, "matched")
            }
            T212ReviewDecision::ImportNew { .. } => {
                let id = uuid::Uuid::new_v4().to_string();
                let ledger_id = format!("t212:{id}");
                insert_financial(user_id, &tx, &ledger_id, &row.financial).await?;
                create_event(user_id, &tx, &id, account.as_deref(), row.financial.kind(), &row.fingerprint, &sources[batch_id], &ledger_id).await?;
                (id, "inserted")
            }
        };
        let count = tx.execute(
            "UPDATE t212_import_row SET event_id=$4,state=$5,review_reason=NULL,resolution=$6,version=version+1
             WHERE user_id=$1 AND batch_id=$2 AND row_number=$3 AND version=$7 AND resolution IS NULL",
            &[&user_id.as_str(), batch_id, ordinal, &event_id, &state, &canonical, &version],
        ).await?;
        ensure!(count == 1, changed());
    }

    for (batch_id, account) in &accounts {
        // Account association covers the file, including already automatic rows.
        // It can fill an unknown association, never move an existing association.
        let links = tx.query(
            "SELECT DISTINCT e.id,e.account_id FROM t212_activity_event e
             JOIN t212_import_row o ON o.user_id=e.user_id AND o.event_id=e.id
             WHERE o.user_id=$1 AND o.batch_id=$2 ORDER BY e.id",
            &[&user_id.as_str(), batch_id],
        ).await?;
        for link in links {
            let current: Option<String> = link.get("account_id");
            ensure!(current.is_none() || &current == account, changed());
            if account.is_some() && current.is_none() {
                let id: String = link.get("id");
                tx.execute("UPDATE t212_activity_event SET account_id=$3 WHERE user_id=$1 AND id=$2 AND account_id IS NULL", &[&user_id.as_str(), &id, account]).await?;
            }
        }
        tx.execute("UPDATE t212_import_batch SET account_id=$3 WHERE user_id=$1 AND id=$2", &[&user_id.as_str(), batch_id, account]).await?;
        trading212_import::finish_batch(user_id, &tx, batch_id).await?;
    }
    let summaries = tx.query(
        "SELECT * FROM t212_import_batch WHERE user_id=$1 AND id=ANY($2) ORDER BY id",
        &[&user_id.as_str(), &batch_ids],
    ).await?.iter().map(store::batch_from_row).collect::<anyhow::Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(T212ReviewResponse { batches: summaries, status: store::status(user_id, &db).await? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import_decision() -> Value {
        json!({"action":"import_new","batch_id":"batch","row_number":1,"version":1,"account_id":null})
    }

    fn match_decision() -> Value {
        json!({"action":"match_existing","batch_id":"batch","row_number":1,"version":1,
            "account_id":null,"kind":"dividend","ledger_id":"retained","accept_recorded_values":false})
    }

    #[test]
    fn decisions_require_explicit_nullable_account_and_match_acknowledgment() {
        for value in [import_decision(), match_decision()] {
            assert!(serde_json::from_value::<T212ReviewDecision>(value.clone()).is_ok());
            let mut owned = value.clone();
            owned["account_id"] = json!("owned-account");
            assert!(serde_json::from_value::<T212ReviewDecision>(owned).is_ok());
            let mut missing = value;
            missing.as_object_mut().unwrap().remove("account_id");
            assert!(serde_json::from_value::<T212ReviewDecision>(missing).is_err());
        }
        let mut missing = match_decision();
        missing.as_object_mut().unwrap().remove("accept_recorded_values");
        assert!(serde_json::from_value::<T212ReviewDecision>(missing).is_err());
        let mut null = match_decision();
        null["accept_recorded_values"] = Value::Null;
        assert!(serde_json::from_value::<T212ReviewDecision>(null).is_err());
    }

    #[test]
    fn review_json_rejects_unknown_fields_and_cross_action_fields() {
        for value in [import_decision(), match_decision()] {
            for key in ["owner_id", "user_id", "discard", "overwrite"] {
                let mut value = value.clone();
                value[key] = json!(true);
                assert!(serde_json::from_value::<T212ReviewDecision>(value).is_err());
            }
        }
        let mut cross_action = import_decision();
        cross_action["ledger_id"] = json!("arbitrary");
        assert!(serde_json::from_value::<T212ReviewDecision>(cross_action).is_err());
        let mut unknown_action = import_decision();
        unknown_action["action"] = json!("discard");
        assert!(serde_json::from_value::<T212ReviewDecision>(unknown_action).is_err());
        assert!(serde_json::from_value::<T212ReviewRequest>(json!({"decisions":[import_decision()],"owner_id":"other"})).is_err());
    }

    #[test]
    fn required_review_identity_fields_are_not_defaulted() {
        for value in [import_decision(), match_decision()] {
            for key in ["batch_id", "row_number", "version", "action"] {
                let mut value = value.clone();
                value.as_object_mut().unwrap().remove(key);
                assert!(serde_json::from_value::<T212ReviewDecision>(value).is_err());
            }
        }
        for key in ["kind", "ledger_id"] {
            let mut value = match_decision();
            value.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<T212ReviewDecision>(value).is_err());
        }
        let mut wrong_account = import_decision();
        wrong_account["account_id"] = json!(123);
        assert!(serde_json::from_value::<T212ReviewDecision>(wrong_account).is_err());
    }

    fn interest_row(amount: Decimal, currency: &str, raw_id: Option<&str>) -> PreparedT212Row {
        let date = DateTime::from_timestamp(1_735_689_600, 0).unwrap();
        let native = json!({"version":1,"kind":"interest","timestamp":canonical_timestamp(date),
            "principal":"Cash","amount":amount.normalize().to_string(),"currency":currency,
            "withholding_tax":"0","withholding_tax_currency":currency});
        PreparedT212Row {
            row_number: 1,
            raw_action: "Interest on cash".into(),
            raw_id: raw_id.map(str::to_owned),
            original_timestamp: canonical_timestamp(date),
            fingerprint: fingerprint_native(&native).unwrap(),
            occurrence: 1,
            native_payload: native,
            financial: T212Financial::Interest(crate::database::models::interest::InterestPayment {
                date, amount, broker: "Trading212".into(), principal: "Cash".into(),
                currency: currency.into(), amount_eur: amount,
                withholding_tax: Decimal::ZERO, withholding_tax_currency: currency.into(),
            }),
        }
    }

    #[test]
    fn changed_raw_id_economics_are_reviewable_but_are_not_arbitrary_ledger_matches() {
        let incoming = interest_row(Decimal::new(2, 0), "EUR", Some("opaque-ID"));
        let recorded = interest_row(Decimal::ONE, "EUR", Some("opaque-ID"));
        let mut candidate = Candidate {
            kind: "interest".into(), ledger_id: "retained-interest".into(),
            event_id: Some("event".into()), account_id: Some("account".into()),
            fingerprint: recorded.fingerprint, financial: recorded.financial,
            observed_fingerprints: HashSet::new(),
            observations: vec![(recorded.raw_id, recorded.native_payload)],
        };
        assert!(evidence_backed(&incoming, &candidate));
        assert!(values_differ(&incoming, &candidate));
        candidate.observations[0].0 = Some("opaque-id".into());
        assert!(!evidence_backed(&incoming, &candidate));
        candidate.observations.clear();
        assert!(!evidence_backed(&incoming, &candidate));
    }

    #[test]
    fn native_bucket_and_recorded_value_comparisons_preserve_currency_and_eur_amounts() {
        let incoming = interest_row(Decimal::ONE, "USD", None);
        let mut candidate = Candidate {
            kind: "interest".into(), ledger_id: "retained-interest".into(),
            event_id: None, account_id: None,
            fingerprint: incoming.fingerprint.clone(), financial: incoming.financial.clone(),
            observed_fingerprints: HashSet::new(),
            observations: Vec::new(),
        };
        assert!(evidence_backed(&incoming, &candidate));
        assert!(!values_differ(&incoming, &candidate));
        if let T212Financial::Interest(recorded) = &mut candidate.financial {
            recorded.amount_eur = Decimal::new(95, 2);
        }
        assert!(values_differ(&incoming, &candidate));
        let other_currency = interest_row(Decimal::ONE, "EUR", None);
        candidate.fingerprint = other_currency.fingerprint;
        candidate.financial = other_currency.financial;
        assert!(evidence_backed(&incoming, &candidate));
        assert!(values_differ(&incoming, &candidate));
    }

    #[test]
    fn resolution_replay_ignores_only_the_version_precondition() {
        let original = match_decision();
        let mut refreshed = original.clone();
        refreshed["version"] = json!(2);
        assert!(same_resolution(&original, &refreshed));
        for (key, value) in [("account_id", json!("account")), ("ledger_id", json!("other")),
            ("accept_recorded_values", json!(true)), ("action", json!("import_new"))] {
            let mut incompatible = refreshed.clone();
            incompatible[key] = value;
            assert!(!same_resolution(&original, &incompatible));
        }
    }
}
