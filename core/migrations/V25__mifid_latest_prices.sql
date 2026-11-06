-- Global parser checkpoints deliberately do not reuse portfolio-universe hashes.
ALTER TABLE market_data_file RENAME COLUMN universe_hash TO parser_version;
DROP TABLE trading_price_watchlist;

-- Older reports have unknown quantity; do not infer an indicative status for them.
ALTER TABLE market_price_report ADD COLUMN quantity NUMERIC;
CREATE INDEX market_price_report_latest
    ON market_price_report(isin, currency, executed_ns DESC, published_ns DESC, id DESC)
    WHERE eligible AND revision <> 2 AND price > 0;
-- Streaming imports stamp their final source-file key only after parsing succeeds.
CREATE INDEX market_price_report_file ON market_price_report(feed, file_key);

CREATE TABLE mifid_latest_price (
    isin TEXT NOT NULL,
    currency TEXT NOT NULL,
    report_id TEXT REFERENCES market_price_report(id),
    PRIMARY KEY (isin, currency)
);
CREATE INDEX mifid_latest_price_report ON mifid_latest_price(report_id)
    WHERE report_id IS NOT NULL;

INSERT INTO mifid_latest_price(isin, currency, report_id)
SELECT DISTINCT ON (r.isin, r.currency) r.isin, r.currency, r.id
FROM market_price_report r
WHERE r.eligible AND r.revision <> 2 AND r.price > 0 AND r.executed_at <= now()
    AND r.published_ns <= (extract(epoch FROM now()) * 1000000000)::bigint
    AND NOT EXISTS (
        SELECT 1 FROM stock_split s WHERE s.isin = r.isin
            AND s.ex_date > r.executed_at AND s.ex_date <= now())
ORDER BY r.isin, r.currency, r.executed_ns DESC, r.published_ns DESC, r.id DESC;
