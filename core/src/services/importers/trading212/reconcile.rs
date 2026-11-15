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
    pub fn native(&self) -> Value { financial_native(&self.financial) }
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

/// Search raw, retained ledger values, never split-adjusted timeline output or
/// listing aliases. Include raw-ID/legacy-hash hits even if their time changed.
pub(crate) async fn candidates_for_rows(
    user_id: &UserId, tx: &Transaction<'_>, incoming: &[PreparedT212Row],
) -> anyhow::Result<Vec<Candidate>> {
    let times: Vec<DateTime<Utc>> = incoming.iter().map(|r| r.financial.date()).collect::<HashSet<_>>().into_iter().collect();
    let raw_ids: Vec<String> = incoming.iter().filter_map(|r| r.raw_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    let legacy_hashes: Vec<String> = raw_ids.iter().map(|id| hash_string(&format!("Trading212{id}"))).collect();
    let mut candidates = Vec::new();
    for (table, key, link, kind) in [("trade","hash","trade_hash","trade"),("dividend","id","dividend_id","dividend"),("interest","id","interest_id","interest"),("fx_conversion","id","fx_conversion_id","fx")] {
        // Table/key/link are closed server-side literals.
        let sql = format!("SELECT l.*,e.id AS event_id,e.account_id FROM {table} l
            LEFT JOIN t212_activity_event e ON e.user_id=l.user_id AND e.{link}=l.{key}
            WHERE l.user_id=$1 AND l.broker='Trading212' AND
              (l.date=ANY($2) OR l.{key}=ANY($3) OR e.id IN
                (SELECT o.event_id FROM t212_import_row o WHERE o.user_id=$1 AND o.raw_id=ANY($4)))
            ORDER BY l.{key}");
        for row in tx.query(&sql, &[&user_id.as_str(), &times, &legacy_hashes, &raw_ids]).await? {
            let currency = if kind != "fx" { row.get::<_, String>("currency") } else { String::new() };
            let financial = match kind {
                "trade" => T212Financial::Trade(Trade { broker:row.get("broker"),date:row.get("date"),units:row.get("units"),avg_price_per_unit:row.get("avg_price_per_unit"),eur_avg_price_per_unit:row.get("eur_avg_price_per_unit"),security_type:row.get("security_type"),direction:row.get("direction"),currency:currency.clone(),isin:row.get("isin"),date_added:row.get("date_added"),fees:row.get("fees"),withholding_tax:tax(&row),withholding_tax_currency:tax_currency(&row,&currency) }),
                "dividend" => T212Financial::Dividend(Dividend { isin:row.get("isin"),date:row.get("date"),amount:row.get("amount"),broker:row.get("broker"),currency:currency.clone(),amount_eur:row.get("amount_eur"),withholding_tax:tax(&row),withholding_tax_currency:tax_currency(&row,&currency) }),
                "interest" => T212Financial::Interest(InterestPayment { date:row.get("date"),amount:row.get("amount"),broker:row.get("broker"),principal:row.get("principal"),currency:currency.clone(),amount_eur:row.get("amount_eur"),withholding_tax:tax(&row),withholding_tax_currency:tax_currency(&row,&currency) }),
                _ => T212Financial::Fx(FxConversion { date:row.get("date"),broker:row.get("broker"),from_amount:row.get("from_amount"),to_amount:row.get("to_amount"),from_currency:row.get("from_currency"),to_currency:row.get("to_currency"),date_added:row.get("date_added"),fees:row.get::<_, Option<Decimal>>("fees").unwrap_or_default() }),
            };
            let event_id: Option<String> = row.get("event_id");
            let mut observed_fingerprints=HashSet::new();
            let observations = if let Some(id) = &event_id {
                tx.query("SELECT DISTINCT raw_id,native_payload,fingerprint FROM t212_import_row WHERE user_id=$1 AND event_id=$2 AND state IN ('matched','inserted')", &[&user_id.as_str(), id]).await?
                    .into_iter().map(|r| { observed_fingerprints.insert(r.get("fingerprint")); (r.get("raw_id"),r.get("native_payload")) }).collect()
            } else { Vec::new() };
            candidates.push(Candidate { kind:kind.into(),ledger_id:row.get(key),event_id,account_id:row.get("account_id"),fingerprint:fingerprint_native(&financial_native(&financial))?,financial,observed_fingerprints,observations });
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
    kind: &str, fingerprint: &str, source: &str, ledger_id: &str,
) -> anyhow::Result<()> {
    let imported_at = if source == "legacy" { None } else { Some(Utc::now()) };
    let trade = (kind=="trade").then_some(ledger_id);
    let dividend = (kind=="dividend").then_some(ledger_id);
    let interest = (kind=="interest").then_some(ledger_id);
    let fx = (kind=="fx").then_some(ledger_id);
    tx.execute("INSERT INTO t212_activity_event(user_id,id,account_id,kind,fingerprint,created_via,imported_at,trade_hash,dividend_id,interest_id,fx_conversion_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        &[&user_id.as_str(),&event_id,&account_id,&kind,&fingerprint,&source,&imported_at,&trade,&dividend,&interest,&fx]).await?;
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

fn evidence_conflicts(row: &PreparedT212Row, candidate: &Candidate) -> bool {
    if candidate.observations.is_empty() {
        // Retained EUR fees and native withholding are the only historical
        // evidence available. Derived EUR principal values do not define identity.
        return match (&row.financial, &candidate.financial) {
            (T212Financial::Trade(a),T212Financial::Trade(b)) => a.fees!=b.fees || a.withholding_tax!=b.withholding_tax || (a.withholding_tax!=Decimal::ZERO && a.withholding_tax_currency!=b.withholding_tax_currency),
            (T212Financial::Dividend(a),T212Financial::Dividend(b)) => a.withholding_tax!=b.withholding_tax || (a.withholding_tax!=Decimal::ZERO && a.withholding_tax_currency!=b.withholding_tax_currency),
            (T212Financial::Interest(a),T212Financial::Interest(b)) => a.withholding_tax!=b.withholding_tax || (a.withholding_tax!=Decimal::ZERO && a.withholding_tax_currency!=b.withholding_tax_currency),
            (T212Financial::Fx(a),T212Financial::Fx(b)) => a.fees!=b.fees,
            _ => true,
        };
    }
    !candidate.observations.iter().any(|(_, native)| {
        ["fees","fees_currency","withholding_tax","withholding_tax_currency"].iter().all(|key| {
            let amount_key = match *key { "fees_currency"=>"fees", "withholding_tax_currency"=>"withholding_tax", other=>other };
            let a = row.native_payload.get(amount_key).and_then(Value::as_str).unwrap_or("0");
            let b = native.get(amount_key).and_then(Value::as_str).unwrap_or("0");
            if key.ends_with("currency") && a=="0" && b=="0" { true }
            else { row.native_payload.get(*key)==native.get(*key) }
        })
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
            let complete_extension = context.source==T212ImportSource::Trading212Api && observed.iter().all(|(_,c)| incoming.iter().any(|r| r.raw_id.as_deref()==Some(raw_id) && c.matches_fingerprint(&r.fingerprint)));
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
    let candidates = candidates_for_rows(user_id,&tx,&prepared.rows).await?;
    let mut used = HashSet::new();
    // Digest shortcuts preserve the actual row-to-event association, not merely
    // the file's former success flag. Deleted ledger links are never a replay.
    let replay = tx.query("SELECT DISTINCT ON (o.row_number) o.row_number,o.event_id,e.account_id
        FROM t212_import_batch b JOIN t212_import_row o ON o.user_id=b.user_id AND o.batch_id=b.id
        JOIN t212_activity_event e ON e.user_id=o.user_id AND e.id=o.event_id
        WHERE b.user_id=$1 AND b.content_hash=$2 AND
          (b.account_id IS NOT DISTINCT FROM $3::text OR (b.account_id IS NULL AND $3::text IS NOT NULL))
        ORDER BY o.row_number,(b.id=$4) DESC,b.created_at", &[&user_id.as_str(),&prepared.content_hash,&context.account_id,&batch_id]).await?;
    let replay: BTreeMap<i32,(String,Option<String>)> = replay.into_iter().map(|r|(r.get(0),(r.get(1),r.get(2)))).collect();
    for row in &prepared.rows {
        // A retry of this very durable batch retains accepted review decisions.
        if let Some(prior) = tx.query_opt("SELECT event_id,state FROM t212_import_row WHERE user_id=$1 AND batch_id=$2 AND row_number=$3 AND event_id IS NOT NULL", &[&user_id.as_str(),&batch_id,&row.row_number]).await? {
            let event:String=prior.get("event_id");
            if let Some(c)=candidates.iter().find(|c|c.event_id.as_ref()==Some(&event)) { used.insert((c.kind.as_str(),c.ledger_id.as_str())); }
            continue;
        }
        let replay_match = replay.get(&row.row_number).and_then(|(event, account)| {
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
                    create_event(user_id,&tx,&id,context.account_id.as_deref(),&candidate.kind,&candidate.fingerprint,"legacy",&candidate.ledger_id).await?;
                    id
                };
                used.insert((candidate.kind.as_str(),candidate.ledger_id.as_str()));
                record_observation(user_id,&tx,&batch_id,row,Some(&event_id),"matched",None).await?;
            }
            Reconciliation::New => {
                let event_id=uuid::Uuid::new_v4().to_string();
                let ledger_id=format!("t212:{event_id}");
                insert_financial(user_id,&tx,&ledger_id,&row.financial).await?;
                create_event(user_id,&tx,&event_id,context.account_id.as_deref(),row.financial.kind(),&row.fingerprint,context.source.as_str(),&ledger_id).await?;
                record_observation(user_id,&tx,&batch_id,row,Some(&event_id),"inserted",None).await?;
            }
            Reconciliation::Review(reason) => record_observation(user_id,&tx,&batch_id,row,None,"review",Some(reason)).await?,
        }
    }
    let outcome=trading212_import::finish_batch(user_id,&tx,&batch_id).await?;
    tx.commit().await?;
    Ok(outcome)
}
