use std::collections::{BTreeMap, HashSet};

use anyhow::{ensure, Context};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use tokio_postgres::{Client, Row, Transaction};

use super::{canonical_decimal as decimal, canonical_timestamp, fingerprint_native, PreparedT212Batch, PreparedT212Row, T212Financial};
use crate::{
    database::{models::{auth::UserId, dividend::Dividend, fx_conversion::FxConversion, instrument::Instrument, interest::InterestPayment, trade::Trade},
        queries::{composite, dividend, fx_conversion, instrument, interest, trading212_import}},
    services::{shared::util::hash_string, trading::sync::{T212ImportContext, T212ImportOutcome, T212ImportSource}},
};

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub kind: String,
    pub ledger_id: String,
    pub event_id: Option<String>,
    pub account_id: Option<String>,
    pub fingerprint: String,
    pub financial: T212Financial,
    pub withholding_tax_reported: bool,
    pub accepted_financial: Vec<T212Financial>,
    pub observed_fingerprints: HashSet<String>,
    pub observations: Vec<(Option<String>, Value)>,
}

impl Candidate {
    pub fn identity(&self) -> String { format!("{}:{}", self.kind, self.ledger_id) }
    pub fn matches_fingerprint(&self, fingerprint: &str) -> bool {
        self.fingerprint==fingerprint || self.observed_fingerprints.contains(fingerprint)
    }
    pub fn has_raw_id(&self, raw_id: &str) -> bool {
        self.observations.iter().any(|(id, _)| id.as_deref()==Some(raw_id))
    }
    pub fn native(&self) -> Value {
        let mut native = financial_native(&self.financial);
        if !self.withholding_tax_reported { native["withholding_tax_reported"] = json!(false); }
        native
    }
}

pub(crate) fn financial_native(financial: &T212Financial) -> Value {
    let mut value = match financial {
        T212Financial::Trade(t) => json!({"isin":t.isin,"direction":t.direction,"units":decimal(t.units),"unit_price":decimal(t.avg_price_per_unit),"currency":t.currency,
            "fees":decimal(t.fees),"fees_currency":"EUR","withholding_tax":decimal(t.withholding_tax),"withholding_tax_currency":t.withholding_tax_currency}),
        T212Financial::Dividend(d) => json!({"isin":d.isin,"amount":decimal(d.amount),"currency":d.currency,"withholding_tax":decimal(d.withholding_tax),"withholding_tax_currency":d.withholding_tax_currency}),
        T212Financial::Interest(i) => json!({"principal":i.principal,"amount":decimal(i.amount),"currency":i.currency,"withholding_tax":decimal(i.withholding_tax),"withholding_tax_currency":i.withholding_tax_currency}),
        T212Financial::Fx(f) => json!({"from_amount":decimal(f.from_amount),"to_amount":decimal(f.to_amount),"from_currency":f.from_currency,"to_currency":f.to_currency,"fees":decimal(f.fees),"fees_currency":"EUR"}),
    };
    value["version"] = json!(1);
    value["kind"] = json!(financial.kind());
    value["timestamp"] = json!(canonical_timestamp(financial.date()));
    value
}

fn tax(row: &Row) -> Decimal { row.get::<_, Option<Decimal>>("withholding_tax").unwrap_or_default() }
fn tax_currency(row: &Row, fallback: &str) -> String { row.get::<_, Option<String>>("withholding_tax_currency").unwrap_or_else(|| fallback.into()) }

/// Recover old interest-only history pages by their exact generated CSV digest.
/// Run phases and zero amounts alone cannot establish unreported withholding.
async fn recover_history_interest(user_id: &UserId, tx: &Transaction<'_>, event_id: &str) -> anyhow::Result<bool> {
    const OLD_HEADER: &str = "Action,Time,ISIN,No. of shares,Price / share,Currency (Price / share),Total,Currency (Total),ID,Currency conversion fee,Exchange rate,Withholding tax,Currency (Withholding tax)\n";
    let batches = tx.query("SELECT DISTINCT b.id,b.content_hash FROM t212_import_batch b
        JOIN t212_import_row o ON o.user_id=b.user_id AND o.batch_id=b.id
        WHERE b.user_id=$1 AND o.event_id=$2 AND o.state='inserted'
          AND b.source='trading212_api' AND b.skipped_count=0
          AND NOT (o.native_payload ? 'withholding_tax_reported')", &[&user_id.as_str(), &event_id]).await?;
    for batch in batches {
        let batch_id: String = batch.get("id");
        let rows = tx.query("SELECT row_number,kind,raw_action,raw_id,original_timestamp,native_payload
            FROM t212_import_row WHERE user_id=$1 AND batch_id=$2 ORDER BY row_number",
            &[&user_id.as_str(), &batch_id]).await?;
        let mut csv = String::from(OLD_HEADER);
        let mut identifiable = !rows.is_empty();
        for (index, row) in rows.iter().enumerate() {
            let native: Value = row.get("native_payload");
            let action: String = row.get("raw_action");
            if row.get::<_, String>("kind") != "interest" || row.get::<_, i32>("row_number") != index as i32 + 1
                || !matches!(action.as_str(), "Interest on cash" | "Lending interest")
                || native.get("withholding_tax_reported").is_some()
                || native.get("withholding_tax").and_then(Value::as_str) != Some("0") {
                identifiable = false; break;
            }
            let timestamp: String = row.get("original_timestamp");
            let raw_id: Option<String> = row.get("raw_id");
            let Some(amount) = native.get("amount").and_then(Value::as_str) else { identifiable = false; break; };
            let Some(currency) = native.get("currency").and_then(Value::as_str) else { identifiable = false; break; };
            let fields = [action.as_str(), timestamp.as_str(), "", "0", "0", "", amount, currency, raw_id.as_deref().unwrap_or(""), "0", "1", "", ""];
            for (index, field) in fields.into_iter().enumerate() {
                if index != 0 { csv.push(','); }
                if field.contains([',', '"', '\n', '\r']) {
                    csv.push('"'); csv.push_str(&field.replace('"', "\"\"")); csv.push('"');
                } else { csv.push_str(field); }
            }
            csv.push('\n');
        }
        if !identifiable || blake3::hash(csv.as_bytes()).to_hex().as_str() != batch.get::<_, String>("content_hash") { continue; }
        tx.execute("UPDATE t212_import_row SET native_payload=native_payload || '{\"withholding_tax_reported\":false}'::jsonb
            WHERE user_id=$1 AND batch_id=$2", &[&user_id.as_str(), &batch_id]).await?;
        tx.execute("UPDATE t212_activity_event e SET withholding_tax_reported=false
            WHERE e.user_id=$1 AND e.kind='interest' AND EXISTS
              (SELECT 1 FROM t212_import_row o WHERE o.user_id=e.user_id AND o.event_id=e.id AND o.batch_id=$2 AND o.state='inserted')
            AND EXISTS (SELECT 1 FROM interest i WHERE i.user_id=e.user_id AND i.id=e.interest_id AND COALESCE(i.withholding_tax,0)=0)
            AND NOT EXISTS (SELECT 1 FROM t212_import_row o WHERE o.user_id=e.user_id AND o.event_id=e.id
              AND o.state IN ('inserted','matched') AND COALESCE(o.native_payload->>'withholding_tax_reported','true') <> 'false')",
            &[&user_id.as_str(), &batch_id]).await?;
    }
    Ok(tx.query_one("SELECT withholding_tax_reported FROM t212_activity_event WHERE user_id=$1 AND id=$2",
        &[&user_id.as_str(), &event_id]).await?.get(0))
}

/// Search raw, retained ledger values, never split-adjusted timeline output or
/// listing aliases. Include raw-ID/legacy-hash hits even if their time changed.
pub(crate) async fn candidates_for_rows(
    user_id: &UserId, tx: &Transaction<'_>, incoming: &[PreparedT212Row],
) -> anyhow::Result<Vec<Candidate>> {
    let times: Vec<DateTime<Utc>> = incoming.iter().map(|r| r.financial.date()).collect::<HashSet<_>>().into_iter().collect();
    let raw_ids: Vec<String> = incoming.iter().filter_map(|r| r.raw_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    let legacy_hashes: Vec<String> = raw_ids.iter().map(|id| hash_string(&format!("Trading212{id}"))).collect();
    let fingerprints: Vec<String> = incoming.iter().map(|r| r.fingerprint.clone()).collect::<HashSet<_>>().into_iter().collect();
    let mut candidates = Vec::new();
    for (table, key, link, kind) in [("trade","hash","trade_hash","trade"),("dividend","id","dividend_id","dividend"),("interest","id","interest_id","interest"),("fx_conversion","id","fx_conversion_id","fx")] {
        // Table/key/link are closed server-side literals.
        let sql = format!("SELECT l.*,e.id AS event_id,e.account_id,COALESCE(e.withholding_tax_reported,true) AS withholding_tax_reported FROM {table} l
            LEFT JOIN t212_activity_event e ON e.user_id=l.user_id AND e.{link}=l.{key}
            WHERE l.user_id=$1 AND l.broker='Trading212' AND
              (l.date=ANY($2) OR l.{key}=ANY($3) OR e.id IN
                (SELECT o.event_id FROM t212_import_row o WHERE o.user_id=$1 AND o.state IN ('matched','inserted')
                  AND (o.raw_id=ANY($4) OR o.fingerprint=ANY($5))))
            ORDER BY l.{key}");
        for row in tx.query(&sql, &[&user_id.as_str(), &times, &legacy_hashes, &raw_ids, &fingerprints]).await? {
            let currency = if kind != "fx" { row.get::<_, String>("currency") } else { String::new() };
            let financial = match kind {
                "trade" => T212Financial::Trade(Trade { broker:row.get("broker"),date:row.get("date"),units:row.get("units"),avg_price_per_unit:row.get("avg_price_per_unit"),eur_avg_price_per_unit:row.get("eur_avg_price_per_unit"),security_type:row.get("security_type"),direction:row.get("direction"),currency:currency.clone(),isin:row.get("isin"),date_added:row.get("date_added"),fees:row.get("fees"),withholding_tax:tax(&row),withholding_tax_currency:tax_currency(&row,&currency) }),
                "dividend" => T212Financial::Dividend(Dividend { isin:row.get("isin"),date:row.get("date"),amount:row.get("amount"),broker:row.get("broker"),currency:currency.clone(),amount_eur:row.get("amount_eur"),withholding_tax:tax(&row),withholding_tax_currency:tax_currency(&row,&currency) }),
                "interest" => T212Financial::Interest(InterestPayment { date:row.get("date"),amount:row.get("amount"),broker:row.get("broker"),principal:row.get("principal"),currency:currency.clone(),amount_eur:row.get("amount_eur"),withholding_tax:tax(&row),withholding_tax_currency:tax_currency(&row,&currency) }),
                _ => T212Financial::Fx(FxConversion { date:row.get("date"),broker:row.get("broker"),from_amount:row.get("from_amount"),to_amount:row.get("to_amount"),from_currency:row.get("from_currency"),to_currency:row.get("to_currency"),date_added:row.get("date_added"),fees:row.get::<_, Option<Decimal>>("fees").unwrap_or_default() }),
            };
            let event_id: Option<String> = row.get("event_id");
            let withholding_tax_reported: bool = row.get("withholding_tax_reported");
            let mut observed_fingerprints=HashSet::new();
            let mut accepted_financial = Vec::new();
            let observations = if let Some(id) = &event_id {
                let stored = tx.query("SELECT DISTINCT raw_id,native_payload,fingerprint,normalized_payload FROM t212_import_row WHERE user_id=$1 AND event_id=$2 AND state IN ('matched','inserted')", &[&user_id.as_str(), id]).await?;
                let mut observations = Vec::with_capacity(stored.len());
                for observation in stored {
                    observed_fingerprints.insert(observation.get("fingerprint"));
                    let normalized: Value = observation.get("normalized_payload");
                    accepted_financial.push(serde_json::from_value(normalized["event"].clone())?);
                    observations.push((observation.get("raw_id"), observation.get("native_payload")));
                }
                observations
            } else { Vec::new() };
            candidates.push(Candidate { kind:kind.into(),ledger_id:row.get(key),event_id,account_id:row.get("account_id"),fingerprint:fingerprint_native(&financial_native(&financial))?,financial,observed_fingerprints,observations,accepted_financial,withholding_tax_reported });
        }
    }
    Ok(candidates)
}

pub(crate) async fn insert_financial(user_id: &UserId, tx: &Transaction<'_>, id: &str, financial: &T212Financial) -> anyhow::Result<()> {
    let inserted = match financial {
        T212Financial::Trade(t) => {
            let inserted = composite::insert_trade_with_id(user_id, tx, id, t).await?;
            if inserted {
                instrument::record_execution_price(user_id, tx, &Instrument { id:t.isin.clone(),last_price_update:t.date,price:t.eur_avg_price_per_unit,name:t.isin.clone() }).await?;
            }
            inserted
        }
        T212Financial::Dividend(d) => dividend::insert_with_id(user_id,tx,id,d).await?,
        T212Financial::Interest(i) => interest::insert_with_id(user_id,tx,id,i).await?,
        T212Financial::Fx(f) => fx_conversion::insert_with_id(user_id,tx,id,f).await?,
    };
    ensure!(inserted, "Trading 212 ledger identity collision; no activity was committed");
    Ok(())
}

pub(crate) async fn create_event(
    user_id: &UserId, tx: &Transaction<'_>, event_id: &str, account_id: Option<&str>,
    kind: &str, fingerprint: &str, source: &str, ledger_id: &str, withholding_tax_reported: bool,
) -> anyhow::Result<()> {
    let imported_at = if source == "legacy" { None } else { Some(Utc::now()) };
    let trade = (kind=="trade").then_some(ledger_id);
    let dividend = (kind=="dividend").then_some(ledger_id);
    let interest = (kind=="interest").then_some(ledger_id);
    let fx = (kind=="fx").then_some(ledger_id);
    tx.execute("INSERT INTO t212_activity_event(user_id,id,account_id,kind,fingerprint,created_via,imported_at,trade_hash,dividend_id,interest_id,fx_conversion_id,withholding_tax_reported)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        &[&user_id.as_str(),&event_id,&account_id,&kind,&fingerprint,&source,&imported_at,&trade,&dividend,&interest,&fx,&withholding_tax_reported]).await?;
    Ok(())
}

pub(crate) async fn record_observation(
    user_id: &UserId, tx: &Transaction<'_>, batch_id: &str, row: &PreparedT212Row,
    event_id: Option<&str>, state: &str, reason: Option<&str>,
) -> anyhow::Result<()> {
    let normalized = json!({"version":1,"event":row.financial});
    tx.execute("INSERT INTO t212_import_row(user_id,batch_id,row_number,kind,raw_action,raw_id,original_timestamp,fingerprint,occurrence,native_payload,normalized_payload,event_id,state,review_reason)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        ON CONFLICT(user_id,batch_id,row_number) DO UPDATE SET event_id=EXCLUDED.event_id,state=EXCLUDED.state,review_reason=EXCLUDED.review_reason",
        &[&user_id.as_str(),&batch_id,&row.row_number,&row.financial.kind(),&row.raw_action,&row.raw_id,&row.original_timestamp,&row.fingerprint,&row.occurrence,&row.native_payload,&normalized,&event_id,&state,&reason]).await?;
    Ok(())
}

pub(crate) fn withholding_reported(row: &PreparedT212Row) -> bool {
    !matches!(row.financial, T212Financial::Dividend(_) | T212Financial::Interest(_))
        || row.native_payload.get("withholding_tax_reported") != Some(&Value::Bool(false))
}

fn tax_equal(a: Decimal, a_currency: &str, b: Decimal, b_currency: &str) -> bool {
    a == b && (a.is_zero() || a_currency == b_currency)
}

pub(crate) fn evidence_conflicts(row: &PreparedT212Row, candidate: &Candidate) -> bool {
    let equal = |other: &T212Financial| match (&row.financial, other) {
        (T212Financial::Trade(a), T212Financial::Trade(b)) =>
            a.fees == b.fees && tax_equal(a.withholding_tax, &a.withholding_tax_currency, b.withholding_tax, &b.withholding_tax_currency),
        (T212Financial::Dividend(a), T212Financial::Dividend(b)) =>
            !withholding_reported(row) || !candidate.withholding_tax_reported
                || tax_equal(a.withholding_tax, &a.withholding_tax_currency, b.withholding_tax, &b.withholding_tax_currency),
        (T212Financial::Interest(a), T212Financial::Interest(b)) =>
            !withholding_reported(row) || !candidate.withholding_tax_reported
                || tax_equal(a.withholding_tax, &a.withholding_tax_currency, b.withholding_tax, &b.withholding_tax_currency),
        (T212Financial::Fx(a), T212Financial::Fx(b)) => a.fees == b.fees,
        _ => false,
    };
    if equal(&candidate.financial) { return false; }
    // Accepted corrections are replay aliases, not permission to overwrite
    // retained values. Compare normalized EUR fees across source encodings.
    !candidate.accepted_financial.iter().zip(&candidate.observations).any(|(financial, (_, native))|
        (candidate.kind == "trade" || candidate.kind == "fx"
            || native.get("withholding_tax_reported") != Some(&Value::Bool(false))) && equal(financial))
}

async fn enrich_withholding(
    user_id: &UserId, tx: &Transaction<'_>, row: &PreparedT212Row, candidate: &Candidate,
) -> anyhow::Result<()> {
    if candidate.withholding_tax_reported || !withholding_reported(row)
        || candidate.fingerprint != row.fingerprint { return Ok(()); }
    let (table, amount, currency) = match &row.financial {
        T212Financial::Dividend(d) => ("dividend", d.withholding_tax, &d.withholding_tax_currency),
        T212Financial::Interest(i) => ("interest", i.withholding_tax, &i.withholding_tax_currency),
        _ => return Ok(()),
    };
    let event_id = candidate.event_id.as_ref().context("Unknown withholding has no durable event")?;
    let sql = format!("UPDATE {table} SET withholding_tax=$3,withholding_tax_currency=$4
        WHERE user_id=$1 AND id=$2 AND COALESCE(withholding_tax,0)=0
          AND EXISTS(SELECT 1 FROM t212_activity_event WHERE user_id=$1 AND id=$5 AND NOT withholding_tax_reported)");
    ensure!(tx.execute(&sql, &[&user_id.as_str(), &candidate.ledger_id, &amount, currency, event_id]).await? == 1,
        "Trading 212 withholding changed before enrichment");
    tx.execute("UPDATE t212_activity_event SET withholding_tax_reported=true WHERE user_id=$1 AND id=$2 AND NOT withholding_tax_reported",
        &[&user_id.as_str(), event_id]).await?;
    Ok(())
}

fn occurrences_cover(observed: &[(usize, &Candidate)], incoming: &[PreparedT212Row], raw_id: &str) -> bool {
    fn assign(index: usize, observed: &[(usize, &Candidate)], incoming: &[PreparedT212Row], raw_id: &str,
        claimed: &mut [Option<usize>], seen: &mut [bool]) -> bool {
        let candidate = observed[index].1;
        for (slot, row) in incoming.iter().enumerate() {
            if seen[slot] || row.raw_id.as_deref() != Some(raw_id)
                || row.financial.kind() != candidate.kind || !candidate.matches_fingerprint(&row.fingerprint) { continue; }
            seen[slot] = true;
            if claimed[slot].is_none() || assign(claimed[slot].unwrap(), observed, incoming, raw_id, claimed, seen) {
                claimed[slot] = Some(index);
                return true;
            }
        }
        false
    }
    let mut claimed = vec![None; incoming.len()];
    let mut seen = vec![false; incoming.len()];
    (0..observed.len()).all(|index| {
        seen.fill(false);
        assign(index, observed, incoming, raw_id, &mut claimed, &mut seen)
    })
}

/// Historical income IDs omitted currency and tax. Equality of that lossy key
/// is review evidence, never sufficient identity for an automatic match.
pub(crate) fn legacy_income_key_matches(row: &PreparedT212Row, candidate: &Candidate) -> bool {
    if candidate.event_id.is_some() { return false; }
    match (&row.financial, &candidate.financial) {
        (T212Financial::Dividend(a), T212Financial::Dividend(b)) =>
            a.isin==b.isin && a.date==b.date && a.amount==b.amount && a.broker==b.broker,
        (T212Financial::Interest(a), T212Financial::Interest(b)) =>
            a.date==b.date && a.amount==b.amount && a.broker==b.broker && a.principal==b.principal,
        _ => false,
    }
}

enum Reconciliation { Match(usize), New, Review(&'static str) }

fn decide(
    row: &PreparedT212Row, candidates: &[Candidate], used: &HashSet<(&str,&str)>,
    context: &T212ImportContext, incoming: &[PreparedT212Row],
) -> Reconciliation {
    let available = |c: &Candidate| !used.contains(&(c.kind.as_str(),c.ledger_id.as_str()));
    let economic = |c: &Candidate| c.kind==row.financial.kind() && c.matches_fingerprint(&row.fingerprint);
    let own = |c: &Candidate| context.account_id.is_some() && c.account_id==context.account_id;
    if context.account_id.is_none() && row.raw_id.as_deref().is_some_and(|raw_id|
        candidates.iter().any(|c| c.kind==row.financial.kind() && c.has_raw_id(raw_id))) {
        return Reconciliation::Review("Observed broker ID requires account and correction confirmation");
    }
    if let Some(raw_id) = row.raw_id.as_deref().filter(|_| context.account_id.is_some()) {
        let observed: Vec<_> = candidates.iter().enumerate().filter(|(_,c)| own(c) && c.kind==row.financial.kind() && c.has_raw_id(raw_id)).collect();
        if let Some((index,candidate)) = observed.iter().find(|(_,c)| economic(c) && available(c)) {
            return if evidence_conflicts(row,candidate) { Reconciliation::Review("Recorded fee or withholding evidence differs") } else { Reconciliation::Match(*index) };
        }
        if !observed.is_empty() && !observed.iter().any(|(_,c)| economic(c)) {
            let complete_extension = context.source==T212ImportSource::Trading212Api && occurrences_cover(&observed, incoming, raw_id);
            if !complete_extension { return Reconciliation::Review("A previously observed broker ID has changed native economics"); }
        }
    }
    let bucket: Vec<_> = candidates.iter().enumerate().filter(|(_,c)| own(c) && economic(c)).collect();
    if let Some((index,candidate)) = bucket.iter().find(|(_,c)| available(c) && (row.raw_id.is_none() || c.observations.iter().all(|(id,_)| id.is_none()))) {
        return if evidence_conflicts(row,candidate) { Reconciliation::Review("Recorded fee or withholding evidence differs") } else { Reconciliation::Match(*index) };
    }
    if bucket.iter().any(|(_,c)| available(c)) {
        return Reconciliation::Review("Equal native economics have disjoint broker IDs; confirm whether these are the same transaction");
    }
    if !bucket.is_empty() && context.source!=T212ImportSource::Trading212Api {
        return Reconciliation::Review("Statement does not establish complete timestamp-bucket cardinality");
    }
    if row.financial.kind()=="trade" {
        if let Some(raw_id) = &row.raw_id {
            let legacy_hash = hash_string(&format!("Trading212{raw_id}"));
            if let Some((index,candidate)) = candidates.iter().enumerate().find(|(_,c)| c.kind=="trade" && c.ledger_id==legacy_hash) {
                if !economic(candidate) || (candidate.account_id.is_some() && candidate.account_id!=context.account_id) {
                    return Reconciliation::Review("Legacy trade ID conflicts with native economics or another account");
                }
                if available(candidate) {
                    return if evidence_conflicts(row,candidate) { Reconciliation::Review("Recorded fee or withholding evidence differs") } else { Reconciliation::Match(index) };
                }
                // Old order hashes collapsed fills. The existing occurrence has
                // already been claimed; additional source occurrences need proof.
                return Reconciliation::Review("Legacy order ID was shared by multiple execution occurrences");
            }
        }
    }
    if candidates.iter().any(|c| legacy_income_key_matches(row,c) || (economic(c) && (c.account_id.is_none() || context.account_id.is_none()))) {
        return Reconciliation::Review("Historical account ownership or occurrence association requires confirmation");
    }
    Reconciliation::New
}

pub(crate) async fn apply_trading212_batch(
    user_id: &UserId, db: &mut Client, mut prepared: PreparedT212Batch, context: &T212ImportContext,
) -> anyhow::Result<T212ImportOutcome> {
    let tx = db.transaction().await?;
    trading212_import::lock_owner(user_id,&tx).await?;
    trading212_import::validate_context(user_id,&tx,context).await?;
    trading212_import::lock_ingestion(user_id,&tx).await?;
    let mut report_from: Option<DateTime<Utc>> = None;
    let mut report_to: Option<DateTime<Utc>> = None;
    if let Some(run) = &context.sync_run_id {
        let row = tx.query_one("SELECT report_from,report_to FROM t212_sync_run WHERE user_id=$1 AND id=$2 FOR UPDATE", &[&user_id.as_str(),run]).await?;
        report_from=row.get("report_from"); report_to=row.get("report_to");
        let from=report_from.context("Sync run has no report interval")?;
        let to=report_to.context("Sync run has no report interval")?;
        prepared.rows.retain(|r| r.financial.date()>=from && r.financial.date()<to);
    }
    let existing_batch = if let Some(job)=&context.statement_job_id {
        tx.query_opt("SELECT id FROM t212_import_batch WHERE user_id=$1 AND statement_job_id=$2 FOR UPDATE", &[&user_id.as_str(),job]).await?
    } else if let Some(run)=&context.sync_run_id {
        tx.query_opt("SELECT id FROM t212_import_batch WHERE user_id=$1 AND sync_run_id=$2 AND content_hash=$3 AND report_from=$4 AND report_to=$5 ORDER BY created_at LIMIT 1 FOR UPDATE", &[&user_id.as_str(),run,&prepared.content_hash,&report_from,&report_to]).await?
    } else {None};
    let batch_id = existing_batch.map(|r|r.get::<_,String>(0)).unwrap_or_else(||uuid::Uuid::new_v4().to_string());
    let skipped_count: i32 = prepared.skipped_actions.values().sum();
    tx.execute("INSERT INTO t212_import_batch(user_id,id,account_id,sync_run_id,statement_job_id,source,content_hash,report_from,report_to,skipped_count,skipped_actions)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT(user_id,id) DO NOTHING",
        &[&user_id.as_str(),&batch_id,&context.account_id,&context.sync_run_id,&context.statement_job_id,&context.source.as_str(),&prepared.content_hash,&report_from,&report_to,&skipped_count,&json!(prepared.skipped_actions)]).await?;
    let mut candidates = candidates_for_rows(user_id,&tx,&prepared.rows).await?;
    for candidate in &mut candidates {
        if let (Some(id), T212Financial::Interest(payment)) = (&candidate.event_id, &candidate.financial) {
            if candidate.withholding_tax_reported && payment.withholding_tax.is_zero() {
                candidate.withholding_tax_reported = recover_history_interest(user_id, &tx, id).await?;
                if !candidate.withholding_tax_reported {
                    for (_, native) in &mut candidate.observations { native["withholding_tax_reported"] = json!(false); }
                }
            }
        }
    }
    let mut used = HashSet::new();
    // Digest shortcuts preserve the actual row-to-event association, not merely
    // the file's former success flag. Deleted ledger links are never a replay.
    let replay = tx.query("SELECT DISTINCT ON (o.row_number) o.row_number,o.event_id,e.account_id,
        COALESCE(o.native_payload->'withholding_tax_reported'='false'::jsonb,false) AS withholding_unreported
        FROM t212_import_batch b JOIN t212_import_row o ON o.user_id=b.user_id AND o.batch_id=b.id
        JOIN t212_activity_event e ON e.user_id=o.user_id AND e.id=o.event_id
        WHERE b.user_id=$1 AND b.content_hash=$2 AND o.state IN ('matched','inserted') AND
          (b.account_id IS NOT DISTINCT FROM $3::text OR (b.account_id IS NULL AND $3::text IS NOT NULL))
        ORDER BY o.row_number,(b.id=$4) DESC,b.created_at", &[&user_id.as_str(),&prepared.content_hash,&context.account_id,&batch_id]).await?;
    let replay: BTreeMap<i32,(String,Option<String>,bool)> = replay.into_iter().map(|r|(r.get(0),(r.get(1),r.get(2),r.get(3)))).collect();
    for row in &mut prepared.rows {
        // Exact-file replays retain recovered history evidence, even though the
        // old file itself predates the explicit unreported-withholding column.
        if replay.get(&row.row_number).is_some_and(|(_, _, unreported)| *unreported)
            && row.native_payload.get("withholding_tax_reported").is_none() {
            row.native_payload["withholding_tax_reported"] = json!(false);
        }
    }
    for row in &prepared.rows {
        // A retry of this very durable batch retains accepted review decisions.
        if let Some(prior) = tx.query_opt("SELECT event_id,state FROM t212_import_row WHERE user_id=$1 AND batch_id=$2 AND row_number=$3 AND event_id IS NOT NULL", &[&user_id.as_str(),&batch_id,&row.row_number]).await? {
            let event:String=prior.get("event_id");
            if let Some(c)=candidates.iter().find(|c|c.event_id.as_ref()==Some(&event)) { used.insert((c.kind.as_str(),c.ledger_id.as_str())); }
            continue;
        }
        let replay_match = replay.get(&row.row_number).and_then(|(event, account, _)| {
            if account.is_some() && account!=&context.account_id {return None;}
            candidates.iter().enumerate().find(|(_,c)|c.event_id.as_ref()==Some(event) && !used.contains(&(c.kind.as_str(),c.ledger_id.as_str())))
        });
        let decision = if let Some((index,candidate))=replay_match {
            let collision = candidate.account_id.is_none() && context.account_id.is_some() && candidates.iter().any(|c|c.account_id==context.account_id && c.kind==row.financial.kind() && c.matches_fingerprint(&row.fingerprint));
            if collision {Reconciliation::Review("Assigning this formerly unassigned file would collide with known account activity")} else {Reconciliation::Match(index)}
        } else {decide(row,&candidates,&used,context,&prepared.rows)};
        match decision {
            Reconciliation::Match(index) => {
                let candidate=&candidates[index];
                let event_id=if let Some(id)=&candidate.event_id {
                    if candidate.account_id.is_none() && context.account_id.is_some() {
                        tx.execute("UPDATE t212_activity_event SET account_id=$3 WHERE user_id=$1 AND id=$2 AND account_id IS NULL", &[&user_id.as_str(),id,&context.account_id]).await?;
                    }
                    id.clone()
                } else {
                    let id=uuid::Uuid::new_v4().to_string();
                    create_event(user_id,&tx,&id,context.account_id.as_deref(),&candidate.kind,&candidate.fingerprint,"legacy",&candidate.ledger_id,true).await?;
                    id
                };
                enrich_withholding(user_id,&tx,row,candidate).await?;
                used.insert((candidate.kind.as_str(),candidate.ledger_id.as_str()));
                record_observation(user_id,&tx,&batch_id,row,Some(&event_id),"matched",None).await?;
            }
            Reconciliation::New => {
                let event_id=uuid::Uuid::new_v4().to_string();
                let ledger_id=format!("t212:{event_id}");
                insert_financial(user_id,&tx,&ledger_id,&row.financial).await?;
                create_event(user_id,&tx,&event_id,context.account_id.as_deref(),row.financial.kind(),&row.fingerprint,context.source.as_str(),&ledger_id,withholding_reported(row)).await?;
                record_observation(user_id,&tx,&batch_id,row,Some(&event_id),"inserted",None).await?;
            }
            Reconciliation::Review(reason) => record_observation(user_id,&tx,&batch_id,row,None,"review",Some(reason)).await?,
        }
    }
    let outcome=trading212_import::finish_batch(user_id,&tx,&batch_id).await?;
    tx.commit().await?;
    Ok(outcome)
}
