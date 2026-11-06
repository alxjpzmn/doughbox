-- Keep V21 immutable: it may already be applied by a running development server.
ALTER TABLE trading_connection
    ADD COLUMN last_submission_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch';
