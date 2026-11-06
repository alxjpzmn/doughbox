-- A missing compact head must not let an older replay become a purported latest trade.
-- Keep ordering provenance, never a synthetic price or market-wide original-report history.
CREATE TABLE mifid_price_floor (
    isin TEXT NOT NULL,
    currency TEXT NOT NULL,
    feed TEXT NOT NULL,
    executed_ns BIGINT NOT NULL,
    published_ns BIGINT NOT NULL,
    report_id TEXT NOT NULL,
    PRIMARY KEY (isin, currency, feed)
);
CREATE INDEX mifid_price_floor_expiry ON mifid_price_floor(executed_ns);

-- Previously published amendments have no original execution provenance: their stored
-- execution may already have moved behind discarded trades. Conservatively withdraw
-- these heads and require a trade at/after this upgrade, rather than bless a replay.
-- Original heads remain usable. The floor expires with the normal quote horizon.
INSERT INTO mifid_price_floor(isin, currency, feed, executed_ns, published_ns, report_id)
SELECT isin, currency, feed,
    GREATEST(executed_ns, (extract(epoch FROM now()) * 1000000000)::bigint),
    published_ns, report_id
FROM mifid_latest_price WHERE revision > 0;
DELETE FROM mifid_latest_price WHERE revision > 0;
