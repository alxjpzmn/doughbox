use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use tokio_postgres::{Client, Transaction};

use super::PriceReport;

pub(crate) struct LatestPrice {
    pub(crate) isin: String,
    pub(crate) currency: String,
    pub(crate) price: Decimal,
    pub(crate) feed: String,
    pub(crate) venue: String,
    pub(crate) as_of: DateTime<Utc>,
    pub(crate) received_at: DateTime<Utc>,
    pub(crate) quantity: Option<Decimal>,
}

pub(crate) async fn latest_prices(client: &Client, isins: &[String]) -> anyhow::Result<Vec<LatestPrice>> {
    let now = Utc::now();
    let cutoff = now - Duration::hours(168);
    let now_ns = now.timestamp_nanos_opt().context("Quote time out of range")?;
    let cutoff_ns = cutoff.timestamp_nanos_opt().context("Quote cutoff out of range")?;
    let rows = client.query(
        "SELECT p.isin, p.currency, p.price, p.feed, p.venue, p.executed_ns, p.received_at, p.quantity
         FROM mifid_latest_price p
         WHERE p.isin = ANY($1) AND p.as_of >= $2 AND p.executed_ns >= $3
            AND p.executed_ns <= $4 AND p.published_ns <= $4 AND p.price > 0
            AND NOT EXISTS (SELECT 1 FROM stock_split s WHERE s.isin = p.isin
                AND s.ex_date > p.as_of AND s.ex_date <= $5)
         ORDER BY p.isin, p.currency, p.feed",
        &[&isins, &cutoff, &cutoff_ns, &now_ns, &now],
    ).await?;
    rows.into_iter().map(|row| Ok(LatestPrice {
        isin: row.try_get("isin")?,
        currency: row.try_get("currency")?,
        price: row.try_get("price")?,
        feed: row.try_get("feed")?,
        venue: row.try_get("venue")?,
        as_of: DateTime::from_timestamp_nanos(row.try_get("executed_ns")?),
        received_at: row.try_get("received_at")?,
        quantity: row.try_get("quantity")?,
    })).collect()
}

pub(super) async fn prepare(tx: &Transaction<'_>) -> anyhow::Result<()> {
    tx.batch_execute(
        "CREATE TEMP TABLE bulk_price_batch (LIKE mifid_latest_price INCLUDING DEFAULTS,
             eligible BOOLEAN NOT NULL, transaction_id TEXT NOT NULL, publisher TEXT NOT NULL) ON COMMIT DROP;
         CREATE TEMP TABLE bulk_price_candidates
             (LIKE mifid_latest_price INCLUDING DEFAULTS INCLUDING CONSTRAINTS) ON COMMIT DROP;
         ALTER TABLE bulk_price_candidates ADD PRIMARY KEY (report_id);
         CREATE TEMP TABLE bulk_price_revisions
             (LIKE mifid_price_revision INCLUDING CONSTRAINTS) ON COMMIT DROP;
         ALTER TABLE bulk_price_revisions ADD PRIMARY KEY (id)",
    ).await?;
    Ok(())
}

/// Retain each report identity until all revisions in the file have been resolved.
/// Temporary candidates may scale with the file; parser batches and permanent heads stay bounded.
pub(super) async fn stage(
    tx: &Transaction<'_>,
    feed: &str,
    reports: &[&PriceReport],
) -> anyhow::Result<()> {
    let ids: Vec<_> = reports.iter().map(|r| r.id.as_str()).collect();
    let isins: Vec<_> = reports.iter().map(|r| r.isin.as_str()).collect();
    let currencies: Vec<_> = reports.iter().map(|r| r.currency.as_str()).collect();
    let venues: Vec<_> = reports.iter().map(|r| r.venue.as_str()).collect();
    let prices: Vec<_> = reports.iter().map(|r| r.price).collect();
    let quantities: Vec<_> = reports.iter().map(|r| r.quantity).collect();
    let executed: Vec<_> = reports.iter().map(|r| r.executed_at).collect();
    let executed_ns: Vec<_> = reports.iter().map(|r| r.executed_ns).collect();
    let published_ns: Vec<_> = reports.iter().map(|r| r.published_ns).collect();
    let revisions: Vec<_> = reports.iter().map(|r| r.revision).collect();
    let hashes: Vec<_> = reports.iter().map(|r| r.revision_hash.as_str()).collect();
    let eligible: Vec<_> = reports.iter().map(|r| r.eligible).collect();
    let transactions: Vec<_> = reports.iter().map(|r| r.transaction_id.as_str()).collect();
    let publishers: Vec<_> = reports.iter().map(|r| r.publisher.as_str()).collect();
    tx.batch_execute("TRUNCATE pg_temp.bulk_price_batch").await?;
    tx.execute(
        "INSERT INTO pg_temp.bulk_price_batch(report_id, isin, currency, venue, price, quantity,
             as_of, executed_ns, published_ns, revision, revision_hash, eligible, transaction_id, publisher, feed)
         SELECT r.*, $15 FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::numeric[],
             $6::numeric[], $7::timestamptz[], $8::bigint[], $9::bigint[], $10::integer[],
             $11::text[], $12::boolean[], $13::text[], $14::text[]) AS r",
        &[&ids, &isins, &currencies, &venues, &prices, &quantities, &executed, &executed_ns,
            &published_ns, &revisions, &hashes, &eligible, &transactions, &publishers, &feed],
    ).await?;
    let now = Utc::now();
    let now_ns = now.timestamp_nanos_opt().context("Quote time out of range")?;
    let cutoff_ns = (now - Duration::hours(168)).timestamp_nanos_opt()
        .context("Quote cutoff out of range")?;
    tx.execute(
        "INSERT INTO pg_temp.bulk_price_revisions(id, published_ns, revision, revision_hash, published_at)
         SELECT report_id, published_ns, revision, revision_hash,
             to_timestamp(published_ns / 1000000000)
                 + (published_ns % 1000000000 / 1000) * interval '1 microsecond'
         FROM pg_temp.bulk_price_batch
         WHERE revision > 0 AND published_ns BETWEEN $1 AND $2
         ON CONFLICT(id) DO UPDATE SET published_ns = EXCLUDED.published_ns,
             revision = EXCLUDED.revision, revision_hash = EXCLUDED.revision_hash,
             published_at = EXCLUDED.published_at
         WHERE (EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash)
             > (bulk_price_revisions.published_ns, bulk_price_revisions.revision, bulk_price_revisions.revision_hash)",
        &[&cutoff_ns, &now_ns],
    ).await?;
    tx.execute(
        "DELETE FROM pg_temp.bulk_price_candidates c USING pg_temp.bulk_price_batch b
         WHERE c.report_id = b.report_id AND b.published_ns <= $1
             AND (b.published_ns, b.revision, b.revision_hash) > (c.published_ns, c.revision, c.revision_hash)",
        &[&now_ns],
    ).await?;
    tx.execute(
        "INSERT INTO pg_temp.bulk_price_candidates(isin, currency, feed, report_id, venue, price,
             quantity, as_of, executed_ns, published_ns, revision, revision_hash, received_at)
         SELECT b.isin, b.currency, b.feed, b.report_id,
             b.venue, b.price, b.quantity, b.as_of, b.executed_ns, b.published_ns, b.revision,
             b.revision_hash, b.received_at
         FROM pg_temp.bulk_price_batch b
         WHERE b.eligible AND b.revision <> 2 AND b.price > 0
             AND b.executed_ns BETWEEN $1 AND $2 AND b.published_ns <= $2
             AND NOT EXISTS (SELECT 1 FROM pg_temp.bulk_price_revisions r WHERE r.id = b.report_id
                 AND (r.published_ns, r.revision, r.revision_hash) > (b.published_ns, b.revision, b.revision_hash))
             AND NOT EXISTS (SELECT 1 FROM mifid_price_revision r WHERE r.id = b.report_id
                 AND (r.published_ns, r.revision, r.revision_hash) > (b.published_ns, b.revision, b.revision_hash))
         ON CONFLICT(report_id) DO UPDATE SET isin = EXCLUDED.isin,
             currency = EXCLUDED.currency, feed = EXCLUDED.feed,
             venue = EXCLUDED.venue, price = EXCLUDED.price, quantity = EXCLUDED.quantity,
             as_of = EXCLUDED.as_of, executed_ns = EXCLUDED.executed_ns, published_ns = EXCLUDED.published_ns,
             revision = EXCLUDED.revision, revision_hash = EXCLUDED.revision_hash, received_at = EXCLUDED.received_at
         WHERE (EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash)
                 > (bulk_price_candidates.published_ns, bulk_price_candidates.revision, bulk_price_candidates.revision_hash)
             OR ((EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash)
                 = (bulk_price_candidates.published_ns, bulk_price_candidates.revision, bulk_price_candidates.revision_hash)
                 AND EXCLUDED.quantity IS DISTINCT FROM bulk_price_candidates.quantity)",
        &[&cutoff_ns, &now_ns],
    ).await?;
    Ok(())
}

/// Called only after the writer takes the shared, short projection lock.
pub(super) async fn apply(tx: &Transaction<'_>) -> anyhow::Result<()> {
    let now = Utc::now();
    let now_ns = now.timestamp_nanos_opt().context("Quote time out of range")?;
    let cutoff_ns = (now - Duration::hours(168)).timestamp_nanos_opt()
        .context("Quote cutoff out of range")?;
    tx.execute(
        "INSERT INTO mifid_price_revision SELECT * FROM pg_temp.bulk_price_revisions ORDER BY id
         ON CONFLICT(id) DO UPDATE SET published_ns = EXCLUDED.published_ns, revision = EXCLUDED.revision,
             revision_hash = EXCLUDED.revision_hash, published_at = EXCLUDED.published_at
         WHERE (EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash)
             > (mifid_price_revision.published_ns, mifid_price_revision.revision, mifid_price_revision.revision_hash)",
        &[],
    ).await?;
    // Losing a persisted head loses the ordering proof for every discarded report,
    // including competitors from earlier files. Replaying its original version
    // cannot establish that a lower runner-up covers all those lost competitors.
    tx.execute(
        "INSERT INTO mifid_price_floor(isin, currency, feed, executed_ns, published_ns, report_id)
         SELECT p.isin, p.currency, p.feed, p.executed_ns, p.published_ns, p.report_id
         FROM mifid_latest_price p
         JOIN pg_temp.bulk_price_revisions incoming ON incoming.id = p.report_id
         JOIN mifid_price_revision r ON r.id = incoming.id
         WHERE (r.published_ns, r.revision, r.revision_hash) > (p.published_ns, p.revision, p.revision_hash)
         ON CONFLICT(isin, currency, feed) DO UPDATE SET executed_ns = EXCLUDED.executed_ns,
             published_ns = EXCLUDED.published_ns, report_id = EXCLUDED.report_id
         WHERE (EXCLUDED.executed_ns, EXCLUDED.published_ns, EXCLUDED.report_id)
             > (mifid_price_floor.executed_ns, mifid_price_floor.published_ns, mifid_price_floor.report_id)",
        &[],
    ).await?;
    tx.execute(
        "DELETE FROM mifid_latest_price p USING pg_temp.bulk_price_revisions incoming, mifid_price_revision r
         WHERE p.report_id = incoming.id AND r.id = incoming.id
             AND (r.published_ns, r.revision, r.revision_hash) > (p.published_ns, p.revision, p.revision_hash)",
        &[],
    ).await?;
    tx.execute(
        "INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price, quantity,
             as_of, executed_ns, published_ns, revision, revision_hash, received_at)
         SELECT DISTINCT ON (c.isin, c.currency, c.feed) c.* FROM pg_temp.bulk_price_candidates c
         WHERE c.executed_ns BETWEEN $1 AND $2 AND c.published_ns <= $2
             AND NOT EXISTS (SELECT 1 FROM mifid_price_revision r WHERE r.id = c.report_id
                 AND (r.published_ns, r.revision, r.revision_hash) > (c.published_ns, c.revision, c.revision_hash))
             AND NOT EXISTS (SELECT 1 FROM mifid_price_floor floor
                 WHERE (floor.isin, floor.currency, floor.feed) = (c.isin, c.currency, c.feed)
                     AND (c.executed_ns, c.published_ns, c.report_id)
                         < (floor.executed_ns, floor.published_ns, floor.report_id))
             AND NOT EXISTS (SELECT 1 FROM stock_split s WHERE s.isin = c.isin
                 AND s.ex_date > c.as_of AND s.ex_date <= $3)
         ORDER BY c.isin, c.currency, c.feed, c.executed_ns DESC, c.published_ns DESC, c.report_id DESC
         ON CONFLICT(isin, currency, feed) DO UPDATE SET report_id = EXCLUDED.report_id,
             venue = EXCLUDED.venue, price = EXCLUDED.price, quantity = EXCLUDED.quantity,
             as_of = EXCLUDED.as_of, executed_ns = EXCLUDED.executed_ns, published_ns = EXCLUDED.published_ns,
             revision = EXCLUDED.revision, revision_hash = EXCLUDED.revision_hash, received_at = EXCLUDED.received_at
         WHERE (EXCLUDED.report_id = mifid_latest_price.report_id
                 AND ((EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash)
                     > (mifid_latest_price.published_ns, mifid_latest_price.revision, mifid_latest_price.revision_hash)
                 OR ((EXCLUDED.published_ns, EXCLUDED.revision, EXCLUDED.revision_hash)
                     = (mifid_latest_price.published_ns, mifid_latest_price.revision, mifid_latest_price.revision_hash)
                     AND EXCLUDED.quantity IS DISTINCT FROM mifid_latest_price.quantity)))
             OR (EXCLUDED.report_id <> mifid_latest_price.report_id
                 AND (EXCLUDED.executed_ns, EXCLUDED.published_ns, EXCLUDED.report_id)
                     > (mifid_latest_price.executed_ns, mifid_latest_price.published_ns, mifid_latest_price.report_id))",
        &[&cutoff_ns, &now_ns, &now],
    ).await?;
    // A defensible head now carries the bound itself. Keep durable floors only
    // for unavailable keys; a later invalidation will preserve the new head's bound.
    tx.execute(
        "DELETE FROM mifid_price_floor f USING pg_temp.bulk_price_candidates c, mifid_latest_price p
         WHERE (f.isin, f.currency, f.feed) = (c.isin, c.currency, c.feed)
             AND (p.isin, p.currency, p.feed) = (f.isin, f.currency, f.feed)
             AND (p.executed_ns, p.published_ns, p.report_id)
                 >= (f.executed_ns, f.published_ns, f.report_id)",
        &[],
    ).await?;
    Ok(())
}
