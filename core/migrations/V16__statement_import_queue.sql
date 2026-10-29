CREATE TABLE statement_import_job (
    id BIGSERIAL PRIMARY KEY,
    filename TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 26214400),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    contents BYTEA,
    queued_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    error TEXT,
    CHECK ((status IN ('queued', 'running') AND contents IS NOT NULL)
        OR (status IN ('succeeded', 'failed') AND contents IS NULL)),
    CHECK ((status IN ('queued', 'running') AND finished_at IS NULL AND error IS NULL)
        OR (status = 'succeeded' AND finished_at IS NOT NULL AND error IS NULL)
        OR (status = 'failed' AND finished_at IS NOT NULL AND error IS NOT NULL))
);

CREATE INDEX statement_import_queued ON statement_import_job (id) WHERE status = 'queued';
CREATE INDEX statement_import_completed ON statement_import_job (id DESC)
    WHERE status IN ('succeeded', 'failed');
