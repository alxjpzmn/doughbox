-- Only user-triggered imports update this metadata; CLI imports still update the data tables.
CREATE TABLE data_import_status (
    source TEXT PRIMARY KEY CHECK (source IN ('polygon', 'ecb')),
    status TEXT NOT NULL DEFAULT 'never_run'
        CHECK (status IN ('never_run', 'running', 'succeeded', 'failed')),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    last_error TEXT
);

INSERT INTO data_import_status(source) VALUES ('polygon'), ('ecb');
