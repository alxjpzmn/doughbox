CREATE TABLE t212_sync_account (
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    id TEXT NOT NULL,
    environment TEXT NOT NULL DEFAULT 'live' CHECK (environment = 'live'),
    broker_account_id TEXT NOT NULL CHECK (btrim(broker_account_id) <> ''),
    currency TEXT NOT NULL,
    schedule_enabled BOOLEAN NOT NULL DEFAULT false,
    next_run_at TIMESTAMPTZ,
    backfill_cursor TIMESTAMPTZ NOT NULL DEFAULT '1970-01-01T00:00:00Z',
    backfill_complete BOOLEAN NOT NULL DEFAULT false,
    synced_through TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, id),
    UNIQUE (user_id, environment, broker_account_id)
);

CREATE TABLE t212_sync_run (
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    connection_generation TEXT NOT NULL,
    trigger TEXT NOT NULL CHECK (trigger IN ('scheduled', 'sync_now', 'full_history')),
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'waiting', 'needs_review', 'succeeded', 'failed', 'cancelled')),
    phase TEXT NOT NULL DEFAULT 'request_report' CHECK (phase IN ('request_report', 'poll_report', 'download_report', 'import_report')),
    range_from TIMESTAMPTZ NOT NULL,
    range_to TIMESTAMPTZ NOT NULL,
    cursor TIMESTAMPTZ NOT NULL,
    report_from TIMESTAMPTZ,
    report_to TIMESTAMPTZ,
    report_id TEXT,
    pending_intervals JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(pending_intervals) = 'array'),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    report_requested_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    error_code TEXT,
    error_message TEXT,
    inserted_count INTEGER NOT NULL DEFAULT 0 CHECK (inserted_count >= 0),
    matched_count INTEGER NOT NULL DEFAULT 0 CHECK (matched_count >= 0),
    review_count INTEGER NOT NULL DEFAULT 0 CHECK (review_count >= 0),
    skipped_count INTEGER NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, account_id) REFERENCES t212_sync_account(user_id, id),
    CHECK (range_from <= cursor AND cursor <= range_to),
    CHECK ((report_from IS NULL) = (report_to IS NULL)),
    CHECK (report_from IS NULL OR (range_from <= report_from AND report_from < report_to AND report_to <= range_to))
);
CREATE UNIQUE INDEX t212_sync_one_active_run ON t212_sync_run(user_id, account_id)
    WHERE status IN ('queued', 'running', 'waiting', 'needs_review');
CREATE INDEX t212_sync_due_run ON t212_sync_run(next_attempt_at)
    WHERE status IN ('queued', 'running', 'waiting');

CREATE TABLE t212_import_batch (
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    id TEXT NOT NULL,
    account_id TEXT,
    sync_run_id TEXT,
    statement_job_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('trading212_api', 'statement_csv')),
    status TEXT NOT NULL DEFAULT 'staged' CHECK (status IN ('staged', 'needs_review', 'applied', 'failed')),
    content_hash TEXT NOT NULL,
    report_from TIMESTAMPTZ,
    report_to TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    applied_at TIMESTAMPTZ,
    inserted_count INTEGER NOT NULL DEFAULT 0 CHECK (inserted_count >= 0),
    matched_count INTEGER NOT NULL DEFAULT 0 CHECK (matched_count >= 0),
    review_count INTEGER NOT NULL DEFAULT 0 CHECK (review_count >= 0),
    skipped_count INTEGER NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    skipped_actions JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(skipped_actions) = 'object'),
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, account_id) REFERENCES t212_sync_account(user_id, id),
    FOREIGN KEY (user_id, sync_run_id) REFERENCES t212_sync_run(user_id, id),
    CHECK (source <> 'trading212_api' OR (account_id IS NOT NULL AND sync_run_id IS NOT NULL)),
    CHECK ((report_from IS NULL) = (report_to IS NULL)),
    CHECK (report_from IS NULL OR report_from < report_to)
);
CREATE UNIQUE INDEX t212_batch_statement_job ON t212_import_batch(user_id, statement_job_id)
    WHERE statement_job_id IS NOT NULL;
CREATE INDEX t212_batch_digest ON t212_import_batch(user_id, account_id, content_hash);
CREATE INDEX t212_batch_pending_review ON t212_import_batch(user_id, created_at)
    WHERE status = 'needs_review';

CREATE TABLE t212_activity_event (
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    id TEXT NOT NULL,
    account_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('trade', 'dividend', 'interest', 'fx')),
    fingerprint TEXT NOT NULL,
    created_via TEXT NOT NULL CHECK (created_via IN ('legacy', 'statement_csv', 'trading212_api')),
    imported_at TIMESTAMPTZ,
    trade_hash TEXT,
    dividend_id TEXT,
    interest_id TEXT,
    fx_conversion_id TEXT,
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, account_id) REFERENCES t212_sync_account(user_id, id),
    FOREIGN KEY (user_id, trade_hash) REFERENCES trade(user_id, hash) ON DELETE CASCADE,
    FOREIGN KEY (user_id, dividend_id) REFERENCES dividend(user_id, id) ON DELETE CASCADE,
    FOREIGN KEY (user_id, interest_id) REFERENCES interest(user_id, id) ON DELETE CASCADE,
    FOREIGN KEY (user_id, fx_conversion_id) REFERENCES fx_conversion(user_id, id) ON DELETE CASCADE,
    CHECK (num_nonnulls(trade_hash, dividend_id, interest_id, fx_conversion_id) = 1),
    CHECK ((kind = 'trade') = (trade_hash IS NOT NULL)),
    CHECK ((kind = 'dividend') = (dividend_id IS NOT NULL)),
    CHECK ((kind = 'interest') = (interest_id IS NOT NULL)),
    CHECK ((kind = 'fx') = (fx_conversion_id IS NOT NULL)),
    CHECK (created_via <> 'legacy' OR imported_at IS NULL)
);
CREATE UNIQUE INDEX t212_event_trade ON t212_activity_event(user_id, trade_hash);
CREATE UNIQUE INDEX t212_event_dividend ON t212_activity_event(user_id, dividend_id);
CREATE UNIQUE INDEX t212_event_interest ON t212_activity_event(user_id, interest_id);
CREATE UNIQUE INDEX t212_event_fx ON t212_activity_event(user_id, fx_conversion_id);
CREATE INDEX t212_event_native_bucket ON t212_activity_event(user_id, account_id, kind, fingerprint);

CREATE TABLE t212_import_row (
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    batch_id TEXT NOT NULL,
    row_number INTEGER NOT NULL CHECK (row_number > 0),
    kind TEXT NOT NULL CHECK (kind IN ('trade', 'dividend', 'interest', 'fx')),
    raw_action TEXT NOT NULL,
    raw_id TEXT CHECK (raw_id IS NULL OR btrim(raw_id) <> ''),
    original_timestamp TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    occurrence INTEGER NOT NULL CHECK (occurrence > 0),
    native_payload JSONB NOT NULL CHECK (native_payload->>'version' = '1'),
    normalized_payload JSONB NOT NULL CHECK (normalized_payload->>'version' = '1'),
    event_id TEXT,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'matched', 'inserted', 'review')),
    review_reason TEXT,
    resolution JSONB,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    PRIMARY KEY (user_id, batch_id, row_number),
    FOREIGN KEY (user_id, batch_id) REFERENCES t212_import_batch(user_id, id),
    FOREIGN KEY (user_id, event_id) REFERENCES t212_activity_event(user_id, id) ON DELETE SET NULL (event_id)
);
CREATE INDEX t212_row_raw_identity ON t212_import_row(user_id, kind, raw_id) WHERE raw_id IS NOT NULL;
CREATE INDEX t212_row_fingerprint ON t212_import_row(user_id, kind, fingerprint);
CREATE INDEX t212_row_event ON t212_import_row(user_id, event_id) WHERE event_id IS NOT NULL;

DO $$
DECLARE private_table TEXT;
BEGIN
    FOREACH private_table IN ARRAY ARRAY[
        't212_sync_account', 't212_sync_run', 't212_import_batch',
        't212_activity_event', 't212_import_row'
    ] LOOP
        EXECUTE format('CREATE TRIGGER private_owner_immutable BEFORE UPDATE ON %I '
            'FOR EACH ROW EXECUTE FUNCTION reject_private_owner_change()', private_table);
    END LOOP;
END;
$$;

CREATE FUNCTION reject_t212_creation_change() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.created_via IS DISTINCT FROM OLD.created_via OR NEW.imported_at IS DISTINCT FROM OLD.imported_at THEN
        RAISE EXCEPTION 'Trading 212 event creation attribution cannot change';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER t212_creation_immutable BEFORE UPDATE ON t212_activity_event
    FOR EACH ROW EXECUTE FUNCTION reject_t212_creation_change();

INSERT INTO t212_sync_account(user_id, id, broker_account_id, currency)
SELECT user_id, gen_random_uuid()::text, account->>'id', COALESCE(account->>'currency', '')
FROM trading_connection
WHERE environment = 'live' AND credentials IS NOT NULL
    AND jsonb_typeof(account->'id') = 'string' AND btrim(account->>'id') <> '';

ALTER TABLE statement_import_job ADD COLUMN trading212_account_id TEXT,
    ADD CONSTRAINT statement_import_t212_account FOREIGN KEY (user_id, trading212_account_id)
        REFERENCES t212_sync_account(user_id, id);
