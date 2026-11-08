-- Apply atomically through the migration runner, with all legacy writers stopped.
-- Column-specific SET NULL is required to retain ownership of unlinked interest.
DO $$
BEGIN
    IF current_setting('server_version_num')::integer < 150000 THEN
        RAISE EXCEPTION 'Doughbox multi-user migration requires PostgreSQL 15 or newer';
    END IF;
END;
$$;

CREATE TABLE auth_users (
    id TEXT PRIMARY KEY,
    username TEXT UNIQUE CHECK (username ~ '^[a-z0-9][a-z0-9_.-]{2,31}$'),
    password_hash TEXT,
    role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'disabled')),
    totp_secret_encrypted TEXT,
    last_totp_step BIGINT,
    auth_version BIGINT NOT NULL DEFAULT 1 CHECK (auth_version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (status <> 'active' OR (
        username IS NOT NULL AND password_hash IS NOT NULL AND totp_secret_encrypted IS NOT NULL
    ))
);

CREATE TABLE auth_instance (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    legacy_owner_user_id TEXT NOT NULL REFERENCES auth_users(id),
    legacy_files_migrated BOOLEAN NOT NULL DEFAULT FALSE,
    legacy_telegram_migrated BOOLEAN NOT NULL DEFAULT FALSE
);

-- Generate once in this transaction, even for an empty installation. Neither
-- runtime callers nor subsequent financial inserts have an implicit owner.
WITH reserved_owner AS (
    INSERT INTO auth_users (id, role, status)
    VALUES (gen_random_uuid()::text, 'admin', 'pending')
    RETURNING id
)
INSERT INTO auth_instance (singleton, legacy_owner_user_id)
SELECT TRUE, id FROM reserved_owner;

-- Links precede sessions because browser link bindings have a real foreign key.
CREATE TABLE auth_links (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('bootstrap', 'invite', 'password_reset', 'operator_recovery')),
    user_id TEXT REFERENCES auth_users(id),
    target_auth_version BIGINT,
    issued_by_user_id TEXT REFERENCES auth_users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    CHECK (
        (kind = 'invite' AND user_id IS NULL AND target_auth_version IS NULL)
        OR (kind <> 'invite' AND user_id IS NOT NULL AND target_auth_version IS NOT NULL)
    )
);
CREATE INDEX auth_links_owner ON auth_links (user_id);
CREATE INDEX auth_links_issuer ON auth_links (issued_by_user_id)
    WHERE consumed_at IS NULL AND revoked_at IS NULL;
CREATE INDEX auth_links_expiry ON auth_links (expires_at);

CREATE TABLE auth_sessions (
    id_hash TEXT PRIMARY KEY,
    stage TEXT NOT NULL CHECK (stage IN ('anonymous', 'mfa_required', 'enrollment_required', 'authenticated')),
    revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    user_id TEXT REFERENCES auth_users(id),
    auth_version BIGINT,
    csrf_token TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    absolute_expires_at TIMESTAMPTZ NOT NULL,
    reauthenticated_at TIMESTAMPTZ,
    failed_factor_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_factor_attempts >= 0),
    pending_totp_id TEXT,
    pending_totp_secret_encrypted TEXT,
    pending_totp_expires_at TIMESTAMPTZ,
    link_id TEXT REFERENCES auth_links(id),
    link_bound_until TIMESTAMPTZ,
    CHECK (
        (stage = 'anonymous' AND user_id IS NULL AND auth_version IS NULL)
        OR (stage <> 'anonymous' AND user_id IS NOT NULL AND auth_version IS NOT NULL)
    ),
    CHECK (reauthenticated_at IS NULL OR stage = 'authenticated'),
    CHECK (
        (pending_totp_id IS NULL AND pending_totp_secret_encrypted IS NULL AND pending_totp_expires_at IS NULL)
        OR (pending_totp_id IS NOT NULL AND pending_totp_secret_encrypted IS NOT NULL AND pending_totp_expires_at IS NOT NULL)
    ),
    CHECK ((link_id IS NULL) = (link_bound_until IS NULL)),
    CHECK (expires_at <= absolute_expires_at)
);
CREATE INDEX auth_sessions_owner ON auth_sessions (user_id, expires_at);
CREATE INDEX auth_sessions_expiry ON auth_sessions (expires_at);
CREATE INDEX auth_sessions_link ON auth_sessions (link_id) WHERE link_id IS NOT NULL;

CREATE TABLE auth_recovery_codes (
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    code_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    used_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, code_hash)
);

CREATE TABLE auth_api_tokens (
    id UUID PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES auth_users(id),
    token_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL CHECK (name = btrim(name) AND char_length(name) BETWEEN 1 AND 80),
    scope TEXT NOT NULL CHECK (scope IN ('read', 'write')),
    auth_version BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ
);
CREATE INDEX auth_api_tokens_owner ON auth_api_tokens (user_id, expires_at)
    WHERE revoked_at IS NULL;
CREATE INDEX auth_api_tokens_expiry ON auth_api_tokens (expires_at);

CREATE TABLE auth_rate_limits (
    bucket_key TEXT NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    attempts INTEGER NOT NULL CHECK (attempts >= 0),
    PRIMARY KEY (bucket_key, window_start)
);
CREATE INDEX auth_rate_limits_expiry ON auth_rate_limits (window_start);

CREATE TABLE user_notification_settings (
    user_id TEXT PRIMARY KEY REFERENCES auth_users(id),
    telegram_chat_id TEXT
);

-- Suspend only the known V9 triggers: archived historical children must not be
-- revalidated, and owner backfill must not increment activity revisions. Internal
-- foreign-key triggers and operator-installed triggers remain enabled.
ALTER TABLE asset DISABLE TRIGGER validate_asset_metadata;
ALTER TABLE asset_trade DISABLE TRIGGER validate_asset_trade;
ALTER TABLE asset_trade DISABLE TRIGGER track_asset_trade_activity;
ALTER TABLE asset_trade DISABLE TRIGGER asset_trade_nonnegative;
ALTER TABLE asset_valuation DISABLE TRIGGER validate_asset_valuation;
ALTER TABLE asset_valuation DISABLE TRIGGER track_asset_valuation_activity;
ALTER TABLE asset_transaction DISABLE TRIGGER validate_asset_transaction;
ALTER TABLE asset_transaction DISABLE TRIGGER track_asset_transaction_activity;
ALTER TABLE asset_transaction DISABLE TRIGGER asset_transaction_nonnegative;
ALTER TABLE asset_balance_snapshot DISABLE TRIGGER validate_asset_snapshot;
ALTER TABLE asset_balance_snapshot DISABLE TRIGGER track_asset_snapshot_activity;
ALTER TABLE asset_balance_snapshot DISABLE TRIGGER asset_snapshot_nonnegative;
ALTER TABLE interest DISABLE TRIGGER validate_asset_interest;
ALTER TABLE interest DISABLE TRIGGER track_asset_interest_activity;
ALTER TABLE interest DISABLE TRIGGER asset_interest_nonnegative;

DO $$
DECLARE
    private_table TEXT;
    reserved_owner_id TEXT;
BEGIN
    SELECT legacy_owner_user_id INTO STRICT reserved_owner_id FROM auth_instance WHERE singleton;
    FOREACH private_table IN ARRAY ARRAY[
        'trade', 'dividend', 'interest', 'fx_conversion', 'tax_optimizations',
        'performance', 'asset', 'asset_trade', 'asset_valuation',
        'asset_transaction', 'asset_balance_snapshot', 'instrument',
        'calculation_status', 'statement_import_job', 'telegram_report_schedule',
        'trading_connection', 'trading_proposals'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN user_id TEXT REFERENCES auth_users(id)', private_table);
        EXECUTE format('UPDATE %I SET user_id = $1', private_table) USING reserved_owner_id;
    END LOOP;
END;
$$;

-- Table renames in V2 retained their original constraint names.
ALTER TABLE trade DROP CONSTRAINT trades_hash_key,
    ADD CONSTRAINT trade_user_hash_key UNIQUE (user_id, hash);
ALTER TABLE dividend DROP CONSTRAINT dividends_pkey,
    ADD PRIMARY KEY (user_id, id);
ALTER TABLE interest DROP CONSTRAINT interest_pkey,
    ADD PRIMARY KEY (user_id, id);
ALTER TABLE fx_conversion DROP CONSTRAINT fx_conversions_id_key,
    ADD CONSTRAINT fx_conversion_user_id_key UNIQUE (user_id, id);
ALTER TABLE tax_optimizations DROP CONSTRAINT tax_optimizations_pkey,
    ADD PRIMARY KEY (user_id, id);
ALTER TABLE instrument DROP CONSTRAINT instruments_id_key,
    ADD CONSTRAINT instrument_user_id_key UNIQUE (user_id, id);

-- Keep generated IDs globally unique, but require the child's owner to match.
ALTER TABLE asset ADD CONSTRAINT asset_user_id_key UNIQUE (user_id, id);
ALTER TABLE asset_trade DROP CONSTRAINT asset_trade_asset_id_fkey,
    ADD CONSTRAINT asset_trade_owner_asset_fkey
        FOREIGN KEY (user_id, asset_id) REFERENCES asset(user_id, id) ON DELETE CASCADE;
ALTER TABLE asset_valuation DROP CONSTRAINT asset_valuation_asset_id_fkey,
    ADD CONSTRAINT asset_valuation_owner_asset_fkey
        FOREIGN KEY (user_id, asset_id) REFERENCES asset(user_id, id) ON DELETE CASCADE;
ALTER TABLE asset_transaction DROP CONSTRAINT asset_transaction_asset_id_fkey,
    ADD CONSTRAINT asset_transaction_owner_asset_fkey
        FOREIGN KEY (user_id, asset_id) REFERENCES asset(user_id, id) ON DELETE CASCADE;
ALTER TABLE asset_balance_snapshot DROP CONSTRAINT asset_balance_snapshot_asset_id_fkey,
    ADD CONSTRAINT asset_snapshot_owner_asset_fkey
        FOREIGN KEY (user_id, asset_id) REFERENCES asset(user_id, id) ON DELETE CASCADE;
-- Delay parsing the PostgreSQL 15 syntax until after the prerequisite check,
-- including when the driver submits the entire migration as one query string.
DO $$
BEGIN
    EXECUTE 'ALTER TABLE interest DROP CONSTRAINT interest_asset_id_fkey,
        ADD CONSTRAINT interest_owner_asset_fkey
        FOREIGN KEY (user_id, asset_id) REFERENCES asset(user_id, id) ON DELETE SET NULL (asset_id)';
END;
$$;

ALTER TABLE calculation_status DROP CONSTRAINT calculation_status_pkey,
    ADD PRIMARY KEY (user_id, source);
ALTER TABLE telegram_report_schedule DROP COLUMN id,
    ADD PRIMARY KEY (user_id);
ALTER TABLE trading_connection DROP COLUMN singleton,
    ADD PRIMARY KEY (user_id),
    ADD COLUMN agent_auth_version BIGINT;
-- Legacy automation credentials have no captured account authority. Broker
-- ciphertext, generation, pacing state, catalogs and proposals stay untouched.
UPDATE trading_connection SET agent_token_hash = NULL, agent_auth_version = NULL;
ALTER TABLE trading_connection ADD CONSTRAINT trading_agent_version_pair
    CHECK ((agent_token_hash IS NULL) = (agent_auth_version IS NULL));
CREATE UNIQUE INDEX trading_connection_agent_hash ON trading_connection (agent_token_hash)
    WHERE agent_token_hash IS NOT NULL;
ALTER TABLE trading_proposals DROP CONSTRAINT trading_proposals_generation_client_request_id_key,
    ADD CONSTRAINT trading_proposals_owner_replay_key UNIQUE (user_id, generation, client_request_id);

DROP INDEX asset_trade_asset_date_idx;
CREATE INDEX asset_trade_asset_date_idx ON asset_trade (user_id, asset_id, date DESC);
DROP INDEX asset_valuation_asset_date_idx;
CREATE INDEX asset_valuation_asset_date_idx ON asset_valuation (user_id, asset_id, date DESC);
DROP INDEX asset_transaction_asset_date_idx;
CREATE INDEX asset_transaction_asset_date_idx ON asset_transaction (user_id, asset_id, date DESC);
DROP INDEX asset_balance_snapshot_asset_date_idx;
CREATE INDEX asset_balance_snapshot_asset_date_idx ON asset_balance_snapshot (user_id, asset_id, date DESC);
DROP INDEX asset_balance_snapshot_one_opening_idx;
CREATE UNIQUE INDEX asset_balance_snapshot_one_opening_idx
    ON asset_balance_snapshot (user_id, asset_id) WHERE kind = 'Opening';
DROP INDEX asset_balance_snapshot_one_reconciliation_at_time_idx;
CREATE UNIQUE INDEX asset_balance_snapshot_one_reconciliation_at_time_idx
    ON asset_balance_snapshot (user_id, asset_id, date) WHERE kind = 'Reconciliation';
DROP INDEX interest_asset_date_idx;
CREATE INDEX interest_asset_date_idx ON interest (user_id, asset_id, date DESC)
    WHERE asset_id IS NOT NULL;
DROP INDEX statement_import_completed;
CREATE INDEX statement_import_completed ON statement_import_job (user_id, id DESC)
    WHERE status IN ('succeeded', 'failed');
-- The existing global queued-id index intentionally remains for the global FIFO.
CREATE INDEX statement_import_owner_active ON statement_import_job (user_id, id)
    WHERE status IN ('queued', 'running');
DROP INDEX trading_proposals_review;
CREATE INDEX trading_proposals_review ON trading_proposals (user_id, status, created_at DESC);
CREATE INDEX trade_owner_date ON trade (user_id, date);
CREATE INDEX trade_owner_isin ON trade (user_id, isin);
CREATE INDEX dividend_owner_date ON dividend (user_id, date);
CREATE INDEX interest_owner_date ON interest (user_id, date);
CREATE INDEX fx_conversion_owner_date ON fx_conversion (user_id, date);
CREATE INDEX tax_optimizations_owner_date ON tax_optimizations (user_id, date);
CREATE INDEX performance_owner_date ON performance (user_id, date);
CREATE INDEX asset_owner_created ON asset (user_id, created_at, id);

-- Replace both ledger helpers, removing the obsolete unscoped signatures.
CREATE FUNCTION assert_asset_trade_nonnegative(target_user_id TEXT, target_asset_id TEXT)
RETURNS VOID AS $$
DECLARE
    minimum_inventory NUMERIC;
BEGIN
    PERFORM 1 FROM asset
    WHERE user_id = target_user_id AND id = target_asset_id FOR NO KEY UPDATE;
    SELECT MIN(running_inventory) INTO minimum_inventory
    FROM (
        SELECT SUM(CASE WHEN direction = 'Buy' THEN units ELSE -units END)
            OVER (ORDER BY date, CASE WHEN direction = 'Buy' THEN 0 ELSE 1 END, created_at, id)
            AS running_inventory
        FROM asset_trade
        WHERE user_id = target_user_id AND asset_id = target_asset_id
    ) inventory;

    IF COALESCE(minimum_inventory, 0) < 0 THEN
        RAISE EXCEPTION 'asset trade ledger cannot have negative inventory';
    END IF;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION validate_asset_trade_ledger()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM assert_asset_trade_nonnegative(OLD.user_id, OLD.asset_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM assert_asset_trade_nonnegative(NEW.user_id, NEW.asset_id);
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION assert_asset_balance_nonnegative(target_user_id TEXT, target_asset_id TEXT)
RETURNS VOID AS $$
DECLARE
    current_class TEXT;
    current_balance NUMERIC := 0;
    opening_date TIMESTAMP WITH TIME ZONE;
    entry RECORD;
BEGIN
    SELECT asset_class INTO current_class FROM asset
    WHERE user_id = target_user_id AND id = target_asset_id FOR NO KEY UPDATE;
    IF current_class IS NULL OR current_class NOT IN ('CashAccount', 'PrivateDebt') THEN
        RETURN;
    END IF;

    SELECT date INTO opening_date
    FROM asset_balance_snapshot
    WHERE user_id = target_user_id AND asset_id = target_asset_id AND kind = 'Opening';

    FOR entry IN
        SELECT * FROM (
            SELECT date,
                CASE kind WHEN 'Opening' THEN 0 ELSE 3 END AS event_order,
                created_at,
                id,
                kind,
                balance AS amount
            FROM asset_balance_snapshot
            WHERE user_id = target_user_id AND asset_id = target_asset_id
                AND current_class = 'CashAccount'
                AND (opening_date IS NULL OR date >= opening_date)
            UNION ALL
            SELECT date,
                CASE kind
                    WHEN 'Deposit' THEN 1
                    WHEN 'PrincipalAdvance' THEN 1
                    ELSE 2
                END AS event_order,
                created_at,
                id,
                kind,
                amount
            FROM asset_transaction
            WHERE user_id = target_user_id AND asset_id = target_asset_id
                AND (opening_date IS NULL OR date >= opening_date)
            UNION ALL
            SELECT date, 1 AS event_order, date AS created_at, id, 'Interest' AS kind, amount
            FROM interest
            WHERE user_id = target_user_id AND asset_id = target_asset_id
                AND current_class = 'CashAccount'
                AND (opening_date IS NULL OR date >= opening_date)
        ) events
        ORDER BY date, event_order, created_at, id
    LOOP
        current_balance := CASE entry.kind
            WHEN 'Opening' THEN entry.amount
            WHEN 'Reconciliation' THEN entry.amount
            WHEN 'Deposit' THEN current_balance + entry.amount
            WHEN 'PrincipalAdvance' THEN current_balance + entry.amount
            WHEN 'Interest' THEN current_balance + entry.amount
            ELSE current_balance - entry.amount
        END;
        IF current_balance < 0 THEN
            RAISE EXCEPTION 'asset balance ledger cannot have a negative balance';
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION validate_asset_balance_ledger()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM assert_asset_balance_nonnegative(OLD.user_id, OLD.asset_id);
    END IF;
    IF TG_OP <> 'DELETE' AND (TG_OP = 'INSERT' OR NEW.asset_id IS DISTINCT FROM OLD.asset_id) THEN
        PERFORM assert_asset_balance_nonnegative(NEW.user_id, NEW.asset_id);
    ELSIF TG_OP = 'UPDATE' THEN
        PERFORM assert_asset_balance_nonnegative(NEW.user_id, NEW.asset_id);
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP FUNCTION assert_asset_trade_nonnegative(TEXT);
DROP FUNCTION assert_asset_balance_nonnegative(TEXT);

CREATE OR REPLACE FUNCTION validate_asset_activity()
RETURNS TRIGGER AS $$
DECLARE
    current_class TEXT;
    current_currency TEXT;
    current_archived_at TIMESTAMP WITH TIME ZONE;
BEGIN
    IF NEW.asset_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT asset_class, currency, archived_at
    INTO current_class, current_currency, current_archived_at
    FROM asset WHERE user_id = NEW.user_id AND id = NEW.asset_id FOR NO KEY UPDATE;

    IF current_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'cannot add or update activity on an archived asset';
    END IF;
    IF TG_TABLE_NAME IN ('asset_trade', 'asset_valuation')
        AND current_class NOT IN ('PhysicalGold', 'RealEstate') THEN
        RAISE EXCEPTION 'trades and valuations require a trade-based asset';
    END IF;
    IF TG_TABLE_NAME = 'asset_transaction'
        AND NOT (
            (current_class = 'CashAccount' AND to_jsonb(NEW)->>'kind' IN ('Deposit', 'Withdrawal'))
            OR (current_class = 'PrivateDebt' AND to_jsonb(NEW)->>'kind' IN ('PrincipalAdvance', 'PrincipalRepayment'))
        ) THEN
        RAISE EXCEPTION 'transaction kind is incompatible with the asset class';
    END IF;
    IF TG_TABLE_NAME = 'asset_balance_snapshot' AND current_class <> 'CashAccount' THEN
        RAISE EXCEPTION 'balance snapshots require a cash account';
    END IF;
    IF TG_TABLE_NAME = 'interest' AND current_class NOT IN ('CashAccount', 'PrivateDebt') THEN
        RAISE EXCEPTION 'linked interest requires a cash account or private debt asset';
    END IF;
    IF TG_TABLE_NAME = 'interest'
        AND (
            (current_class = 'CashAccount' AND to_jsonb(NEW)->>'principal' IS DISTINCT FROM 'Cash')
            OR (current_class = 'PrivateDebt' AND to_jsonb(NEW)->>'principal' IS DISTINCT FROM 'PrivateDebt')
        ) THEN
        RAISE EXCEPTION 'interest principal is incompatible with the asset class';
    END IF;
    IF TG_TABLE_NAME <> 'asset_balance_snapshot'
        AND to_jsonb(NEW)->>'currency' <> current_currency THEN
        RAISE EXCEPTION 'activity currency must match the asset currency';
    END IF;
    IF TG_TABLE_NAME = 'interest' AND current_currency = 'EUR'
        AND (to_jsonb(NEW)->>'amount')::numeric <> (to_jsonb(NEW)->>'amount_eur')::numeric THEN
        RAISE EXCEPTION 'EUR interest must have the same EUR equivalent';
    END IF;
    IF TG_TABLE_NAME = 'asset_balance_snapshot'
        AND current_currency = 'EUR'
        AND (to_jsonb(NEW)->>'balance')::numeric <> (to_jsonb(NEW)->>'balance_eur')::numeric THEN
        RAISE EXCEPTION 'EUR balances must have the same EUR equivalent';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION track_asset_activity()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE asset SET activity_revision = activity_revision + 1
        WHERE user_id = OLD.user_id AND id = OLD.asset_id;
        RETURN OLD;
    ELSIF TG_OP = 'INSERT' THEN
        UPDATE asset SET activity_revision = activity_revision + 1
        WHERE user_id = NEW.user_id AND id = NEW.asset_id;
    ELSIF NEW.asset_id IS DISTINCT FROM OLD.asset_id THEN
        UPDATE asset SET activity_revision = activity_revision + 1
        WHERE user_id = OLD.user_id AND id = OLD.asset_id;
        UPDATE asset SET activity_revision = activity_revision + 1
        WHERE user_id = NEW.user_id AND id = NEW.asset_id;
    ELSE
        UPDATE asset SET activity_revision = activity_revision + 1
        WHERE user_id = NEW.user_id AND id = NEW.asset_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION validate_asset_metadata_update()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.asset_class IN ('CashAccount', 'PrivateDebt') AND NEW.unit_label <> NEW.currency THEN
        RAISE EXCEPTION 'cash and private debt unit labels must match their currency';
    END IF;

    IF (NEW.currency IS DISTINCT FROM OLD.currency OR NEW.unit_label IS DISTINCT FROM OLD.unit_label)
        AND (
            EXISTS (SELECT 1 FROM asset_trade WHERE user_id = OLD.user_id AND asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM asset_valuation WHERE user_id = OLD.user_id AND asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM asset_transaction WHERE user_id = OLD.user_id AND asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM asset_balance_snapshot WHERE user_id = OLD.user_id AND asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM interest WHERE user_id = OLD.user_id AND asset_id = OLD.id)
        ) THEN
        RAISE EXCEPTION 'asset currency and unit label cannot change after activity is recorded';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Install immutability only after backfill; there is deliberately no owner
-- default for any future financial row, including the reserved owner's rows.
CREATE FUNCTION reject_private_owner_change()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.user_id IS DISTINCT FROM OLD.user_id THEN
        RAISE EXCEPTION 'private row ownership cannot change';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DO $$
DECLARE
    private_table TEXT;
BEGIN
    FOREACH private_table IN ARRAY ARRAY[
        'trade', 'dividend', 'interest', 'fx_conversion', 'tax_optimizations',
        'performance', 'asset', 'asset_trade', 'asset_valuation',
        'asset_transaction', 'asset_balance_snapshot', 'instrument',
        'calculation_status', 'statement_import_job', 'telegram_report_schedule',
        'trading_connection', 'trading_proposals'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ALTER COLUMN user_id SET NOT NULL', private_table);
        EXECUTE format(
            'CREATE TRIGGER private_owner_immutable BEFORE UPDATE ON %I '
            'FOR EACH ROW EXECUTE FUNCTION reject_private_owner_change()', private_table
        );
    END LOOP;
END;
$$;
CREATE TRIGGER private_owner_immutable BEFORE UPDATE ON user_notification_settings
FOR EACH ROW EXECUTE FUNCTION reject_private_owner_change();

ALTER TABLE asset ENABLE TRIGGER validate_asset_metadata;
ALTER TABLE asset_trade ENABLE TRIGGER validate_asset_trade;
ALTER TABLE asset_trade ENABLE TRIGGER track_asset_trade_activity;
ALTER TABLE asset_trade ENABLE TRIGGER asset_trade_nonnegative;
ALTER TABLE asset_valuation ENABLE TRIGGER validate_asset_valuation;
ALTER TABLE asset_valuation ENABLE TRIGGER track_asset_valuation_activity;
ALTER TABLE asset_transaction ENABLE TRIGGER validate_asset_transaction;
ALTER TABLE asset_transaction ENABLE TRIGGER track_asset_transaction_activity;
ALTER TABLE asset_transaction ENABLE TRIGGER asset_transaction_nonnegative;
ALTER TABLE asset_balance_snapshot ENABLE TRIGGER validate_asset_snapshot;
ALTER TABLE asset_balance_snapshot ENABLE TRIGGER track_asset_snapshot_activity;
ALTER TABLE asset_balance_snapshot ENABLE TRIGGER asset_snapshot_nonnegative;
ALTER TABLE interest ENABLE TRIGGER validate_asset_interest;
ALTER TABLE interest ENABLE TRIGGER track_asset_interest_activity;
ALTER TABLE interest ENABLE TRIGGER asset_interest_nonnegative;

-- Supply missing control rows only. Existing schedules, status, timestamps,
-- destinations and broker settings must survive bootstrap and enrollment.
INSERT INTO calculation_status (user_id, source)
SELECT legacy_owner_user_id, source
FROM auth_instance CROSS JOIN (VALUES ('performance'), ('taxation')) AS sources(source)
WHERE singleton
ON CONFLICT (user_id, source) DO NOTHING;
INSERT INTO telegram_report_schedule (user_id)
SELECT legacy_owner_user_id FROM auth_instance WHERE singleton
ON CONFLICT (user_id) DO NOTHING;
INSERT INTO trading_connection (user_id, generation)
SELECT legacy_owner_user_id, 'unconfigured' FROM auth_instance WHERE singleton
ON CONFLICT (user_id) DO NOTHING;
INSERT INTO user_notification_settings (user_id)
SELECT legacy_owner_user_id FROM auth_instance WHERE singleton
ON CONFLICT (user_id) DO NOTHING;

COMMENT ON TABLE auth_instance IS
    'Stable reserved legacy owner and serialized bootstrap/admin authority; never a runtime portfolio fallback';
COMMENT ON COLUMN trading_connection.agent_auth_version IS
    'Account version captured when issuing the agent token; NULL exactly when the hash is NULL';
COMMENT ON COLUMN user_notification_settings.telegram_chat_id IS
    'Private destination; missing values never fall back to another account or TG_CHAT_ID';
