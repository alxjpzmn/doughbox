CREATE TABLE mifid_quote_cache (
    isin TEXT PRIMARY KEY,
    checked_at TIMESTAMPTZ NOT NULL,
    as_of TIMESTAMPTZ,
    state JSONB NOT NULL
);
CREATE INDEX mifid_quote_cache_expiry ON mifid_quote_cache(as_of);
CREATE INDEX mifid_quote_cache_negative_expiry ON mifid_quote_cache(checked_at)
    WHERE as_of IS NULL;
