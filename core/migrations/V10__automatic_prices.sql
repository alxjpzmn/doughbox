-- Existing prices remain usable, but their provenance cannot be inferred safely.
ALTER TABLE instrument
    ADD COLUMN price_source TEXT NOT NULL DEFAULT 'legacy_unknown',
    ADD COLUMN price_venue TEXT,
    ADD COLUMN price_received_at TIMESTAMPTZ,
    ADD COLUMN price_report_id TEXT,
    ADD COLUMN price_pinned BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN fallback_price NUMERIC,
    ADD COLUMN fallback_at TIMESTAMPTZ,
    ADD COLUMN fallback_source TEXT NOT NULL DEFAULT 'legacy_unknown';

UPDATE instrument SET fallback_price = price, fallback_at = last_price_update;

CREATE TABLE price_feed_schedule (
    feed TEXT PRIMARY KEY,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    next_refresh_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_success_at TIMESTAMPTZ,
    last_error TEXT
);

CREATE TABLE background_job (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'refresh_prices' CHECK (kind = 'refresh_prices'),
    feed TEXT NOT NULL REFERENCES price_feed_schedule(feed),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_until TIMESTAMPTZ,
    lease_token BIGINT NOT NULL DEFAULT 0,
    rerun_requested BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    last_error TEXT,
    files_processed INTEGER NOT NULL DEFAULT 0,
    reports_processed INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX background_job_one_active_feed
    ON background_job(feed) WHERE status IN ('queued', 'running');
CREATE INDEX background_job_available ON background_job(available_at)
    WHERE status IN ('queued', 'running');

CREATE TABLE market_data_file (
    feed TEXT NOT NULL,
    file_key TEXT NOT NULL,
    universe_hash TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    etag TEXT,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (feed, file_key, universe_hash)
);
CREATE INDEX market_data_file_retention ON market_data_file(processed_at);

-- One current revision per source-scoped transaction. Keep cancellation tombstones
-- even when the original report has not arrived yet.
CREATE TABLE market_price_report (
    id TEXT PRIMARY KEY,
    feed TEXT NOT NULL,
    isin TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    publisher TEXT NOT NULL,
    venue TEXT NOT NULL,
    executed_at TIMESTAMPTZ NOT NULL,
    executed_ns BIGINT NOT NULL,
    published_ns BIGINT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 2),
    revision_hash TEXT NOT NULL,
    price NUMERIC NOT NULL,
    currency TEXT NOT NULL,
    eligible BOOLEAN NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    file_key TEXT NOT NULL
);
CREATE INDEX market_price_report_selection ON market_price_report(isin, executed_ns DESC)
    WHERE eligible AND revision <> 2;
CREATE INDEX market_price_report_retention ON market_price_report(received_at);
