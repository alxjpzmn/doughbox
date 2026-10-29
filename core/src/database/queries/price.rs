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

use super::background_job::lock_lease;

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

pub fn universe_hash(tracked: &HashSet<String>) -> String {
    let sorted: BTreeSet<_> = tracked.iter().collect();
    let mut hash = blake3::Hasher::new();
    // Reprocess the available window when eligibility rules change, not just tracked ISINs.
    hash.update(b"price-eligibility-v2:zero-quantity\n");
    for isin in sorted {
        hash.update(isin.as_bytes());
        hash.update(b"\n");
    }
    hash.finalize().to_hex().to_string()
}

pub async fn checkpoints(
    client: &Client,
    feed: &str,
    universe: &str,
) -> anyhow::Result<HashMap<String, FileCheckpoint>> {
    Ok(client.query("SELECT file_key, content_hash, etag FROM market_data_file WHERE feed = $1 AND universe_hash = $2",
        &[&feed, &universe]).await?.into_iter().map(|row| (row.get(0), FileCheckpoint { content_hash: row.get(1), etag: row.get(2) })).collect())
}

pub async fn ensure_instruments(client: &Client, tracked: &HashSet<String>) -> anyhow::Result<()> {
    let isins: Vec<_> = tracked.iter().collect();
    client.execute("INSERT INTO instrument(id, name, price, last_price_update, fallback_price, fallback_at)
        SELECT isin, isin, 0, '1970-01-01'::timestamptz, 0, '1970-01-01'::timestamptz FROM unnest($1::text[]) AS isin
        ON CONFLICT(id) DO NOTHING", &[&isins]).await?;
    Ok(())
}

pub async fn commit_file(
    client: &mut Client,
    job: &PriceJob,
    file_key: &str,
    universe: &str,
    content_hash: &str,
    etag: Option<&str>,
    reports: Vec<PriceReport>,
) -> anyhow::Result<usize> {
    // A file may contain multiple revisions of one transaction; deduplicate before
    // INSERT ... ON CONFLICT, which cannot update the same row twice in one statement.
    let mut latest: HashMap<String, PriceReport> = HashMap::new();
    for report in reports {
        let replace = latest.get(&report.id).is_none_or(|old| {
            (report.published_ns, report.revision, &report.revision_hash)
                > (old.published_ns, old.revision, &old.revision_hash)
        });
        if replace {
            latest.insert(report.id.clone(), report);
        }
    }
    let reports: Vec<_> = latest.into_values().collect();
    let tx = client.transaction().await?;
    lock_lease(&tx, job).await?;
    if !reports.is_empty() {
        let ids: Vec<_> = reports.iter().map(|r| &r.id).collect();
        let mut affected: BTreeSet<String> = reports.iter().map(|r| r.isin.clone()).collect();
        for row in tx
            .query(
                "SELECT DISTINCT isin FROM market_price_report WHERE id = ANY($1)",
                &[&ids],
            )
            .await?
        {
            affected.insert(row.get(0));
        }
        let isins: Vec<_> = reports.iter().map(|r| &r.isin).collect();
        let transactions: Vec<_> = reports.iter().map(|r| &r.transaction_id).collect();
        let publishers: Vec<_> = reports.iter().map(|r| &r.publisher).collect();
        let venues: Vec<_> = reports.iter().map(|r| &r.venue).collect();
        let executed: Vec<_> = reports.iter().map(|r| r.executed_at).collect();
        let executed_ns: Vec<_> = reports.iter().map(|r| r.executed_ns).collect();
        let published_ns: Vec<_> = reports.iter().map(|r| r.published_ns).collect();
        let revisions: Vec<_> = reports.iter().map(|r| r.revision).collect();
        let hashes: Vec<_> = reports.iter().map(|r| &r.revision_hash).collect();
        let prices: Vec<_> = reports.iter().map(|r| r.price).collect();
        let currencies: Vec<_> = reports.iter().map(|r| &r.currency).collect();
        let eligible: Vec<_> = reports.iter().map(|r| r.eligible).collect();
        tx.execute("INSERT INTO market_price_report(id, feed, isin, transaction_id, publisher, venue,
            executed_at, executed_ns, published_ns, revision, revision_hash, price, currency, eligible, file_key)
            SELECT r.id, $14, r.isin, r.transaction_id, r.publisher, r.venue, r.executed_at,
                r.executed_ns, r.published_ns, r.revision, r.revision_hash, r.price, r.currency, r.eligible, $15
            FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::timestamptz[],
                $7::bigint[], $8::bigint[], $9::integer[], $10::text[], $11::numeric[], $12::text[], $13::boolean[])
            AS r(id, isin, transaction_id, publisher, venue, executed_at, executed_ns, published_ns,
                revision, revision_hash, price, currency, eligible)
            ON CONFLICT(id) DO UPDATE SET isin = EXCLUDED.isin, venue = EXCLUDED.venue,
                executed_at = EXCLUDED.executed_at, executed_ns = EXCLUDED.executed_ns,
                published_ns = EXCLUDED.published_ns, revision = EXCLUDED.revision,
                revision_hash = EXCLUDED.revision_hash, price = EXCLUDED.price,
                currency = EXCLUDED.currency, eligible = EXCLUDED.eligible,
                received_at = now(), file_key = EXCLUDED.file_key
            WHERE (EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash) >
                  (market_price_report.published_ns, market_price_report.revision, market_price_report.revision_hash)
               OR ((EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash) =
                   (market_price_report.published_ns, market_price_report.revision, market_price_report.revision_hash)
                   AND EXCLUDED.eligible IS DISTINCT FROM market_price_report.eligible)",
            &[&ids, &isins, &transactions, &publishers, &venues, &executed, &executed_ns, &published_ns,
                &revisions, &hashes, &prices, &currencies, &eligible, &job.feed, &file_key]).await?;
        // Sorted row locks prevent different feed workers deadlocking on shared ISINs.
        for isin in affected {
            select_quote(&tx, &isin).await?;
        }
    }
    tx.execute(
        "INSERT INTO market_data_file(feed, file_key, universe_hash, content_hash, etag)
        VALUES($1,$2,$3,$4,$5) ON CONFLICT(feed, file_key, universe_hash) DO UPDATE SET
            content_hash = EXCLUDED.content_hash, etag = EXCLUDED.etag, processed_at = now()",
        &[&job.feed, &file_key, &universe, &content_hash, &etag],
    )
    .await?;
    let count = i32::try_from(reports.len())?;
    tx.execute(
        "UPDATE background_job SET files_processed = files_processed + 1,
        reports_processed = reports_processed + $2 WHERE id = $1",
        &[&job.id, &count],
    )
    .await?;
    // Check again after potentially expensive work; an expired worker may not commit.
    lock_lease(&tx, job).await?;
    tx.commit().await?;
    Ok(reports.len())
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
            "SELECT id, price, last_price_update, price_source, price_venue, price_received_at,
         price_pinned, (NOT price_pinned AND EXISTS(SELECT 1 FROM stock_split s WHERE s.isin = i.id
            AND s.ex_date > i.last_price_update AND s.ex_date <= now())) AS invalid_after_split
         FROM instrument i WHERE id = ANY($1)",
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
                older_than_hour: valid && as_of < Utc::now() - Duration::hours(1),
            },
        );
    }
    Ok(isins
        .iter()
        .map(|isin| {
            quotes.get(isin).cloned().unwrap_or_else(|| PriceQuote {
                isin: isin.clone(),
                price: None,
                currency: "EUR".into(),
                source: "missing".into(),
                venue: None,
                as_of: None,
                received_at: None,
                pinned: false,
                older_than_hour: false,
            })
        })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_fingerprint_is_stable_and_changes_for_new_instruments() {
        let first = HashSet::from(["A".into(), "B".into()]);
        let second = HashSet::from(["B".into(), "A".into()]);
        assert_eq!(universe_hash(&first), universe_hash(&second));
        assert_ne!(
            universe_hash(&first),
            universe_hash(&HashSet::from(["A".into()]))
        );
    }
}
