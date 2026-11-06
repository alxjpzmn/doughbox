-- Market-wide latest trades are independent of portfolio report history.
CREATE TABLE mifid_latest_price (
    isin TEXT NOT NULL,
    currency TEXT NOT NULL,
    feed TEXT NOT NULL,
    report_id TEXT NOT NULL,
    venue TEXT NOT NULL,
    price NUMERIC NOT NULL,
    quantity NUMERIC,
    as_of TIMESTAMPTZ NOT NULL,
    executed_ns BIGINT NOT NULL,
    published_ns BIGINT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 1),
    revision_hash TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (isin, currency, feed)
);
CREATE INDEX mifid_latest_price_expiry ON mifid_latest_price(as_of);
CREATE INDEX mifid_latest_price_identity ON mifid_latest_price(report_id);

-- Only revised identities need a replay guard after their latest price is replaced.
CREATE TABLE mifid_price_revision (
    id TEXT PRIMARY KEY,
    published_ns BIGINT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 2),
    revision_hash TEXT NOT NULL,
    published_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX mifid_price_revision_expiry ON mifid_price_revision(published_at);

INSERT INTO mifid_price_revision(id, published_ns, revision, revision_hash, published_at)
SELECT id, published_ns, revision, revision_hash,
       to_timestamp(published_ns / 1000000000) + (published_ns % 1000000000 / 1000) * interval '1 microsecond'
FROM market_price_report
WHERE revision > 0
    AND published_ns >= (extract(epoch FROM now() - interval '7 days') * 1000000000)::bigint
    AND published_ns <= (extract(epoch FROM now()) * 1000000000)::bigint;

INSERT INTO mifid_latest_price(isin, currency, feed, report_id, venue, price, quantity,
    as_of, executed_ns, published_ns, revision, revision_hash, received_at)
SELECT DISTINCT ON (r.isin, r.currency, r.feed)
    r.isin, r.currency, r.feed, r.id, r.venue, r.price, r.quantity,
    r.executed_at, r.executed_ns, r.published_ns, r.revision, r.revision_hash, r.received_at
FROM market_price_report r
WHERE r.eligible AND r.revision <> 2 AND r.price > 0
    AND r.executed_at >= now() - interval '168 hours' AND r.executed_at <= now()
    AND r.executed_ns >= (extract(epoch FROM now() - interval '168 hours') * 1000000000)::bigint
    AND r.executed_ns <= (extract(epoch FROM now()) * 1000000000)::bigint
    AND r.published_ns <= (extract(epoch FROM now()) * 1000000000)::bigint
    AND NOT EXISTS (SELECT 1 FROM stock_split s WHERE s.isin = r.isin
        AND s.ex_date > r.executed_at AND s.ex_date <= now())
ORDER BY r.isin, r.currency, r.feed, r.executed_ns DESC, r.published_ns DESC, r.id DESC;
