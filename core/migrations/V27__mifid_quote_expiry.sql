-- Index quote event time so expiry scans only the latest-price cache, not trade history.
ALTER TABLE mifid_latest_price ADD COLUMN as_of TIMESTAMPTZ;
UPDATE mifid_latest_price latest SET as_of = report.executed_at
FROM market_price_report report WHERE report.id = latest.report_id;

-- Old/empty references must not survive an upgrade just because no feed job succeeds.
DELETE FROM mifid_latest_price
WHERE as_of IS NULL OR as_of < now() - interval '7 days';

ALTER TABLE mifid_latest_price
    ALTER COLUMN report_id SET NOT NULL,
    ALTER COLUMN as_of SET NOT NULL;
CREATE INDEX mifid_latest_price_expiry ON mifid_latest_price(as_of);
