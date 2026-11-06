use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::ensure;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use tokio_postgres::{Client, Transaction};

use crate::{
    database::queries::listing_change::get_listing_changes_with_client,
    services::{
        instruments::identifiers::get_changed_identifier,
        market_data::prices::{
            config::Feed,
            types::{PriceFeedStatus, PriceJob, PriceQuote, PriceReport},
        },
    },
};

use super::background_job::{lock_lease, LEASE_SECONDS};
mod latest;
pub(crate) use latest::{latest_prices, LatestPrice};

/// Bump when source admission or parser policy changes so available files replay.
pub const PARSER_VERSION: &str = "mifid-bulk-v4";
pub(super) const PROJECTION_LOCK: i64 = 0x4D49464944505243;
const MAX_REPORT_BATCH: usize = 4096;

pub struct FileCheckpoint {
    pub content_hash: String,
    pub etag: Option<String>,
}

pub async fn tracked_isins(client: &Client) -> anyhow::Result<HashSet<String>> {
    // Include closed positions so later corrections remain replayable. Never price a
    // known bond/derivative as an equity merely because a feed reports MONE.
    let changes = get_listing_changes_with_client(client).await?;
    let mut eligible = HashMap::new();
    for row in client
        .query("SELECT DISTINCT isin, security_type FROM trade", &[])
        .await?
    {
        let isin: String = row.get(0);
        let security_type: String = row.get(1);
        let isin = get_changed_identifier(&isin, changes.clone());
        let entry = eligible.entry(isin).or_insert(true);
        *entry &= security_type == "Equity";
    }
    Ok(eligible
        .into_iter()
        .filter_map(|(isin, equity)| equity.then_some(isin))
        .collect())
}

/// A checkpoint is reusable only for the exact portfolio universe pinned by its job.
pub fn portfolio_scope_hash(tracked: &HashSet<String>) -> String {
    let mut isins: Vec<_> = tracked.iter().map(String::as_str).collect();
    isins.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    for isin in isins {
        hasher.update(isin.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex().to_string()
}

fn latest_reports(reports: &[PriceReport]) -> Vec<&PriceReport> {
    // One unnest upsert cannot affect the same ID twice. Borrow the winning
    // revision rather than clone report payloads, and lock source rows in ID order.
    let mut latest: HashMap<&str, &PriceReport> = HashMap::with_capacity(reports.len());
    for report in reports {
        let replace = latest.get(report.id.as_str()).is_none_or(|old| {
            (report.published_ns, report.revision, &report.revision_hash)
                > (old.published_ns, old.revision, &old.revision_hash)
        });
        if replace {
            latest.insert(report.id.as_str(), report);
        }
    }
    let mut reports: Vec<_> = latest.into_values().collect();
    reports.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    reports
}

/// Read-only admissions for one feed refresh, retaining only admitted identities.
#[derive(Default)]
pub struct DryRunAdmissions {
    admitted: HashSet<String>,
}

impl DryRunAdmissions {
    pub async fn push(
        &mut self,
        client: &Client,
        reports: &[PriceReport],
        tracked: &HashSet<String>,
    ) -> anyhow::Result<usize> {
        ensure!(reports.len() <= MAX_REPORT_BATCH, "Price report batch exceeds {MAX_REPORT_BATCH}");
        let reports = latest_reports(reports);
        let ids: Vec<_> = reports.iter()
            .filter(|report| !tracked.contains(&report.isin) && !self.admitted.contains(report.id.as_str()))
            .map(|report| report.id.as_str())
            .collect();
        let rows = if ids.is_empty() {
            Vec::new()
        } else {
            client.query("SELECT id FROM market_price_report WHERE id = ANY($1)", &[&ids]).await?
        };
        let existing: HashSet<&str> = rows.iter().map(|row| row.get("id")).collect();
        let mut count = 0;
        for report in reports {
            if self.admitted.contains(report.id.as_str()) {
                count += 1;
            } else if tracked.contains(&report.isin) || existing.contains(report.id.as_str()) {
                self.admitted.insert(report.id.clone());
                count += 1;
            }
        }
        Ok(count)
    }
}

pub async fn checkpoints(
    client: &Client,
    feed: &str,
    scope_hash: &str,
) -> anyhow::Result<HashMap<String, FileCheckpoint>> {
    Ok(client.query("SELECT file_key, content_hash, etag FROM market_data_file
        WHERE feed = $1 AND parser_version = $2 AND scope_hash = $3",
        &[&feed, &PARSER_VERSION, &scope_hash]).await?.into_iter().map(|row| (row.get(0), FileCheckpoint { content_hash: row.get(1), etag: row.get(2) })).collect())
}

pub async fn ensure_instruments(client: &Client, tracked: &HashSet<String>) -> anyhow::Result<()> {
    let isins: Vec<_> = tracked.iter().collect();
    client.execute("INSERT INTO instrument(id, name, price, last_price_update, fallback_price, fallback_at)
        SELECT isin, isin, 0, '1970-01-01'::timestamptz, 0, '1970-01-01'::timestamptz FROM unnest($1::text[]) AS isin
        ON CONFLICT(id) DO NOTHING", &[&isins]).await?;
    Ok(())
}

/// A dropped writer rolls back every source row, projection and checkpoint in its file.
pub struct FileWriter<'a> {
    tx: Transaction<'a>,
    job: &'a PriceJob,
    file_key: &'a str,
    tracked: &'a HashSet<String>,
    scope_hash: &'a str,
    reports_processed: usize,
}

pub async fn begin_file<'a>(
    client: &'a mut Client,
    job: &'a PriceJob,
    file_key: &'a str,
    tracked: &'a HashSet<String>,
    scope_hash: &'a str,
) -> anyhow::Result<FileWriter<'a>> {
    let tx = client.transaction().await?;
    lock_lease(&tx, job).await?;
    // Keep only distinct affected identities across the file, on the database side.
    // Report payloads and upsert arrays remain bounded by one parser batch.
    tx.batch_execute(
        "CREATE TEMP TABLE portfolio_price_isins (isin TEXT PRIMARY KEY) ON COMMIT DROP",
    )
    .await?;
    latest::prepare(&tx).await?;
    let writer = FileWriter {
        tx,
        job,
        file_key,
        tracked,
        scope_hash,
        reports_processed: 0,
    };
    writer.renew_lease().await?;
    Ok(writer)
}

impl FileWriter<'_> {
    async fn renew_lease(&self) -> anyhow::Result<()> {
        // The transaction owns the job-row lock, so the external heartbeat cannot
        // renew it until this file commits. Never resurrect an already expired lease.
        let changed = self.tx.execute(
            "UPDATE background_job SET lease_until = clock_timestamp() + $3::integer * interval '1 second'
             WHERE id = $1 AND lease_token = $2 AND status = 'running'
                AND lease_until > clock_timestamp()",
            &[&self.job.id, &self.job.lease_token, &LEASE_SECONDS],
        ).await?;
        ensure!(changed == 1, "Price job {} lost its lease", self.job.id);
        Ok(())
    }

    pub async fn push(&mut self, reports: &[PriceReport]) -> anyhow::Result<()> {
        ensure!(reports.len() <= MAX_REPORT_BATCH, "Price report batch exceeds {MAX_REPORT_BATCH}");
        self.renew_lease().await?;
        let mut reports = latest_reports(reports);
        if reports.is_empty() {
            return Ok(());
        }
        latest::stage(&self.tx, &self.job.feed, &reports).await?;
        let mut ids: Vec<_> = reports.iter().map(|&report| report.id.as_str()).collect();
        // Lock only this bounded batch's existing source identities, in the same
        // order as the upsert. Existing identities remain admissible even when a
        // correction moves them away from the pinned portfolio.
        let rows = self.tx.query(
            "SELECT id, isin FROM market_price_report
             WHERE id = ANY($1) ORDER BY id FOR NO KEY UPDATE",
            &[&ids],
        ).await?;
        let existing: HashSet<&str> = rows.iter().map(|row| row.get("id")).collect();
        reports.retain(|report| {
            self.tracked.contains(&report.isin) || existing.contains(report.id.as_str())
        });
        if reports.is_empty() {
            return Ok(());
        }
        ids.clear();
        ids.extend(reports.iter().map(|&report| report.id.as_str()));
        // Both old and new ISINs must be reselected when an admitted identity moves.
        let mut affected: Vec<&str> = Vec::with_capacity(rows.len() + reports.len());
        affected.extend(rows.iter().map(|row| row.get::<_, &str>("isin")));
        affected.extend(reports.iter().map(|report| report.isin.as_str()));
        self.tx.execute(
            "INSERT INTO pg_temp.portfolio_price_isins(isin)
             SELECT isin FROM unnest($1::text[]) AS isin WHERE TRUE ON CONFLICT DO NOTHING",
            &[&affected],
        ).await?;
        self.tx.execute(
            "INSERT INTO market_price_report(id, feed, isin, transaction_id, publisher, venue,
                executed_at, executed_ns, published_ns, revision, revision_hash, price, currency,
                eligible, quantity, file_key)
             SELECT r.report_id, $2, r.isin, r.transaction_id, r.publisher, r.venue, r.as_of,
                r.executed_ns, r.published_ns, r.revision, r.revision_hash, r.price, r.currency,
                r.eligible, r.quantity, $3
             FROM pg_temp.bulk_price_batch r WHERE r.report_id = ANY($1)
             ORDER BY r.report_id
             ON CONFLICT(id) DO UPDATE SET isin = EXCLUDED.isin, venue = EXCLUDED.venue,
                executed_at = EXCLUDED.executed_at, executed_ns = EXCLUDED.executed_ns,
                published_ns = EXCLUDED.published_ns, revision = EXCLUDED.revision,
                revision_hash = EXCLUDED.revision_hash, price = EXCLUDED.price,
                currency = EXCLUDED.currency, eligible = EXCLUDED.eligible, quantity = EXCLUDED.quantity,
                received_at = now(), file_key = EXCLUDED.file_key
             WHERE (EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash) >
                   (market_price_report.published_ns, market_price_report.revision, market_price_report.revision_hash)
                OR ((EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash) =
                    (market_price_report.published_ns, market_price_report.revision, market_price_report.revision_hash)
                    AND (EXCLUDED.eligible IS DISTINCT FROM market_price_report.eligible
                        OR EXCLUDED.quantity IS DISTINCT FROM market_price_report.quantity))",
            &[&ids, &self.job.feed, &self.file_key],
        ).await?;
        self.reports_processed += reports.len();
        self.renew_lease().await?;
        Ok(())
    }

    pub async fn finish(
        self,
        content_hash: &str,
        etag: Option<&str>,
    ) -> anyhow::Result<usize> {
        self.renew_lease().await?;
        // Serialize only the short projection phase, not downloads or source writes.
        // At READ COMMITTED the next statement sees every earlier projection's commit.
        self.tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&PROJECTION_LOCK]).await?;
        self.renew_lease().await?;
        latest::apply(&self.tx).await?;
        for row in self.tx.query(
            "SELECT isin FROM pg_temp.portfolio_price_isins ORDER BY isin",
            &[],
        ).await? {
            let isin: &str = row.get("isin");
            if self.tracked.contains(isin) {
                select_quote(&self.tx, isin).await?;
            }
        }
        self.tx.execute(
            "INSERT INTO market_data_file(feed, file_key, parser_version, scope_hash, content_hash, etag)
             VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(feed, file_key, parser_version, scope_hash) DO UPDATE SET
                content_hash = EXCLUDED.content_hash, etag = EXCLUDED.etag, processed_at = now()",
            &[&self.job.feed, &self.file_key, &PARSER_VERSION, &self.scope_hash, &content_hash, &etag],
        ).await?;
        let count = i32::try_from(self.reports_processed)?;
        self.tx.execute(
            "UPDATE background_job SET files_processed = files_processed + 1,
                reports_processed = reports_processed + $2 WHERE id = $1",
            &[&self.job.id, &count],
        ).await?;
        self.renew_lease().await?;
        self.tx.commit().await?;
        Ok(self.reports_processed)
    }
}

pub async fn select_quote(tx: &Transaction<'_>, isin: &str) -> anyhow::Result<()> {
    let Some(instrument) = tx
        .query_opt(
            "SELECT price_pinned, fallback_price, fallback_at, fallback_source
        FROM instrument WHERE id = $1 FOR UPDATE",
            &[&isin],
        )
        .await?
    else {
        return Ok(());
    };
    if instrument.get::<_, bool>("price_pinned") {
        return Ok(());
    }
    let fallback_price: Option<Decimal> = instrument.get("fallback_price");
    let fallback_at: Option<DateTime<Utc>> = instrument.get("fallback_at");
    let fallback_source: String = instrument.get("fallback_source");
    let split_after_fallback: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM stock_split WHERE isin = $1
        AND ex_date > $2 AND ex_date <= now())",
            &[&isin, &fallback_at],
        )
        .await?
        .get(0);
    let fallback_valid =
        fallback_price.is_some_and(|price| price > Decimal::ZERO) && !split_after_fallback;
    let candidate = tx.query_opt("SELECT id, price, executed_at, venue, received_at, feed FROM market_price_report r
        WHERE isin = $1 AND eligible AND revision <> 2 AND currency = 'EUR' AND price > 0 AND executed_at <= now()
        AND NOT EXISTS (SELECT 1 FROM stock_split s WHERE s.isin = r.isin AND s.ex_date > r.executed_at AND s.ex_date <= now())
        ORDER BY (executed_at >= now() - interval '1 hour') DESC,
            CASE feed WHEN 'DETR-posttrade' THEN 0 WHEN 'DGAT-posttrade' THEN 1
                WHEN 'DFRA-posttrade' THEN 2 WHEN 'CBOE-posttrade' THEN 3
                WHEN 'EURONEXT-posttrade' THEN 4 ELSE 5 END,
            executed_ns DESC, published_ns DESC, id DESC LIMIT 1", &[&isin]).await?;
    if let Some(candidate) = candidate {
        let as_of: DateTime<Utc> = candidate.get("executed_at");
        if !fallback_valid || fallback_at.is_none_or(|date| date <= as_of) {
            let source = Feed::from_key(candidate.get("feed"))?.source();
            tx.execute("UPDATE instrument SET price = $2, last_price_update = $3,
                price_source = $7, price_venue = $4, price_received_at = $5, price_report_id = $6 WHERE id = $1",
                &[&isin, &candidate.get::<_, Decimal>("price"), &as_of, &candidate.get::<_, String>("venue"),
                    &candidate.get::<_, DateTime<Utc>>("received_at"), &candidate.get::<_, String>("id"), &source]).await?;
            return Ok(());
        }
    }
    if fallback_valid {
        tx.execute(
            "UPDATE instrument SET price = $2, last_price_update = $3, price_source = $4,
            price_venue = NULL, price_received_at = NULL, price_report_id = NULL WHERE id = $1",
            &[&isin, &fallback_price, &fallback_at, &fallback_source],
        )
        .await?;
    } else {
        // Preserve the numerical value for audit, but exclude it from valuations.
        tx.execute(
            "UPDATE instrument SET price_source = 'unavailable', price_report_id = NULL,
            price_venue = NULL, price_received_at = NULL WHERE id = $1",
            &[&isin],
        )
        .await?;
    }
    Ok(())
}

pub async fn reselect_all(
    client: &mut Client,
    job: &PriceJob,
    tracked: &HashSet<String>,
) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    lock_lease(&tx, job).await?;
    tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&PROJECTION_LOCK]).await?;
    for isin in tracked.iter().collect::<BTreeSet<_>>() {
        select_quote(&tx, isin).await?;
    }
    lock_lease(&tx, job).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn set_manual(
    client: &mut Client,
    isin: &str,
    price: Decimal,
    as_of: DateTime<Utc>,
) -> anyhow::Result<()> {
    ensure!(price > Decimal::ZERO, "Manual price must be positive");
    ensure!(
        as_of <= Utc::now(),
        "Manual price timestamp cannot be in the future"
    );
    let tx = client.transaction().await?;
    let changed = tx.execute("UPDATE instrument SET price = $2, last_price_update = $3, price_pinned = TRUE,
        price_source = 'manual', price_venue = NULL, price_report_id = NULL, price_received_at = now() WHERE id = $1",
        &[&isin, &price, &as_of]).await?;
    ensure!(changed == 1, "Unknown instrument {isin}; import it first");
    tx.commit().await?;
    Ok(())
}

pub async fn unpin(client: &mut Client, isin: &str) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    let changed = tx
        .execute(
            "UPDATE instrument SET price_pinned = FALSE WHERE id = $1",
            &[&isin],
        )
        .await?;
    ensure!(changed == 1, "Unknown instrument {isin}");
    select_quote(&tx, isin).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn quotes(client: &Client, isins: &[String]) -> anyhow::Result<Vec<PriceQuote>> {
    let rows = client
        .query(
            "SELECT i.id, i.price, last_price_update, price_source, price_venue, price_received_at,
         price_pinned, r.quantity, (NOT price_pinned AND EXISTS(SELECT 1 FROM stock_split s WHERE s.isin = i.id
            AND s.ex_date > i.last_price_update AND s.ex_date <= now())) AS invalid_after_split
         FROM instrument i LEFT JOIN market_price_report r ON r.id = i.price_report_id
         WHERE i.id = ANY($1)",
            &[&isins],
        )
        .await?;
    let mut quotes = HashMap::new();
    for row in rows {
        let isin: String = row.get("id");
        let source: String = if row.get::<_, bool>("invalid_after_split") {
            "unavailable".into()
        } else {
            row.get("price_source")
        };
        let price: Decimal = row.get("price");
        let valid = source != "unavailable" && price > Decimal::ZERO;
        let as_of: DateTime<Utc> = row.get("last_price_update");
        quotes.insert(
            isin.clone(),
            PriceQuote {
                isin,
                price: valid.then_some(price),
                currency: "EUR".into(),
                source,
                venue: row.get("price_venue"),
                as_of: valid.then_some(as_of),
                received_at: row.get("price_received_at"),
                pinned: row.get("price_pinned"),
                indicative: if valid && !row.get::<_, bool>("price_pinned") {
                    row.get::<_, Option<Decimal>>("quantity").map(|quantity| quantity.is_zero())
                } else {
                    None
                },
                older_than_hour: valid && as_of < Utc::now() - Duration::hours(1),
            },
        );
    }
    Ok(ordered_quotes(isins, &quotes))
}

fn missing_quote(isin: String, currency: String) -> PriceQuote {
    PriceQuote {
        isin,
        price: None,
        currency,
        source: "missing".into(),
        venue: None,
        as_of: None,
        received_at: None,
        pinned: false,
        indicative: None,
        older_than_hour: false,
    }
}

fn ordered_quotes(isins: &[String], quotes: &HashMap<String, PriceQuote>) -> Vec<PriceQuote> {
    isins
        .iter()
        .map(|isin| {
            quotes.get(isin).cloned().unwrap_or_else(|| missing_quote(isin.clone(), "EUR".into()))
        })
        .collect()
}

pub async fn feed_status(client: &Client) -> anyhow::Result<Vec<PriceFeedStatus>> {
    let rows = client.query("SELECT s.feed, s.enabled, s.next_refresh_at, s.last_success_at,
        COALESCE(j.last_error, s.last_error) AS last_error, j.status, j.attempts, j.files_processed, j.reports_processed,
        (s.enabled AND COALESCE(c.enabled, FALSE)) AS scheduled
        FROM price_feed_schedule s LEFT JOIN price_scheduler_settings c ON c.singleton
        LEFT JOIN LATERAL
            (SELECT * FROM background_job WHERE feed = s.feed ORDER BY id DESC LIMIT 1) j ON TRUE ORDER BY s.feed", &[]).await?;
    rows.iter()
        .map(|row| {
            let scheduled: bool = row.try_get("scheduled")?;
            Ok(PriceFeedStatus {
                feed: row.try_get("feed")?,
                enabled: row.try_get("enabled")?,
                scheduled,
                next_refresh_at: scheduled.then(|| row.get("next_refresh_at")),
                last_success_at: row.try_get("last_success_at")?,
                last_error: row.try_get("last_error")?,
                job_status: row.try_get("status")?,
                attempts: row.try_get::<_, Option<i32>>("attempts")?.unwrap_or(0),
                files_processed: row
                    .try_get::<_, Option<i32>>("files_processed")?
                    .unwrap_or(0),
                reports_processed: row
                    .try_get::<_, Option<i32>>("reports_processed")?
                    .unwrap_or(0),
            })
        })
        .collect()
}

