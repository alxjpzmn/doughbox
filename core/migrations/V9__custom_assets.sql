CREATE TABLE asset (
    id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    name TEXT NOT NULL CHECK (btrim(name) <> ''),
    asset_class TEXT NOT NULL CHECK (
        asset_class IN ('PhysicalGold', 'RealEstate', 'PrivateDebt', 'CashAccount')
    ),
    currency TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    unit_label TEXT NOT NULL CHECK (btrim(unit_label) <> ''),
    current_interest_rate_percent NUMERIC,
    interest_rate_updated_at TIMESTAMP WITH TIME ZONE,
    tax_treatment TEXT NOT NULL DEFAULT 'Excluded' CHECK (tax_treatment = 'Excluded'),
    archived_at TIMESTAMP WITH TIME ZONE,
    activity_revision BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
    CHECK (
        current_interest_rate_percent IS NULL
        OR current_interest_rate_percent BETWEEN 0 AND 100
    ),
    CHECK (
        (asset_class = 'CashAccount' AND current_interest_rate_percent IS NOT NULL)
        OR (asset_class <> 'CashAccount' AND current_interest_rate_percent IS NULL)
    ),
    CHECK (asset_class NOT IN ('CashAccount', 'PrivateDebt') OR unit_label = currency)
);

CREATE TABLE asset_trade (
    id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    asset_id TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    units NUMERIC NOT NULL CHECK (units > 0),
    price_per_unit NUMERIC NOT NULL CHECK (price_per_unit >= 0),
    eur_price_per_unit NUMERIC NOT NULL CHECK (eur_price_per_unit >= 0),
    direction TEXT NOT NULL CHECK (direction IN ('Buy', 'Sell')),
    currency TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    broker TEXT NOT NULL CHECK (btrim(broker) <> ''),
    fees NUMERIC NOT NULL DEFAULT 0 CHECK (fees >= 0),
    fees_eur NUMERIC NOT NULL DEFAULT 0 CHECK (fees_eur >= 0),
    note TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
    CHECK (
        (price_per_unit = 0 AND eur_price_per_unit = 0)
        OR (price_per_unit > 0 AND eur_price_per_unit > 0)
    ),
    CHECK (currency <> 'EUR' OR price_per_unit = eur_price_per_unit),
    CHECK (currency <> 'EUR' OR fees = fees_eur)
);

CREATE TABLE asset_valuation (
    id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    asset_id TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    price_per_unit NUMERIC NOT NULL CHECK (price_per_unit >= 0),
    eur_price_per_unit NUMERIC NOT NULL CHECK (eur_price_per_unit >= 0),
    currency TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    source TEXT NOT NULL CHECK (source IN ('Manual', 'Trade')),
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
    CHECK (
        (price_per_unit = 0 AND eur_price_per_unit = 0)
        OR (price_per_unit > 0 AND eur_price_per_unit > 0)
    ),
    CHECK (currency <> 'EUR' OR price_per_unit = eur_price_per_unit)
);

CREATE TABLE asset_transaction (
    id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    asset_id TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN ('Deposit', 'Withdrawal', 'PrincipalAdvance', 'PrincipalRepayment')
    ),
    amount NUMERIC NOT NULL CHECK (amount > 0),
    amount_eur NUMERIC NOT NULL CHECK (amount_eur > 0),
    currency TEXT NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    note TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
    CHECK (currency <> 'EUR' OR amount = amount_eur)
);

CREATE TABLE asset_balance_snapshot (
    id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    asset_id TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('Opening', 'Reconciliation')),
    balance NUMERIC NOT NULL CHECK (balance >= 0),
    balance_eur NUMERIC NOT NULL CHECK (balance_eur >= 0),
    note TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
    CHECK (
        (balance = 0 AND balance_eur = 0)
        OR (balance > 0 AND balance_eur > 0)
    )
);

ALTER TABLE interest
    ADD COLUMN asset_id TEXT REFERENCES asset(id) ON DELETE SET NULL,
    ADD COLUMN tax_treatment TEXT NOT NULL DEFAULT 'Included'
        CHECK (tax_treatment IN ('Included', 'Excluded')),
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'Imported'
        CHECK (origin IN ('Imported', 'Manual'));

CREATE INDEX asset_trade_asset_date_idx ON asset_trade (asset_id, date DESC);
CREATE INDEX asset_valuation_asset_date_idx ON asset_valuation (asset_id, date DESC);
CREATE INDEX asset_transaction_asset_date_idx ON asset_transaction (asset_id, date DESC);
CREATE INDEX asset_balance_snapshot_asset_date_idx
    ON asset_balance_snapshot (asset_id, date DESC);
CREATE UNIQUE INDEX asset_balance_snapshot_one_opening_idx
    ON asset_balance_snapshot (asset_id) WHERE kind = 'Opening';
CREATE UNIQUE INDEX asset_balance_snapshot_one_reconciliation_at_time_idx
    ON asset_balance_snapshot (asset_id, date) WHERE kind = 'Reconciliation';
CREATE INDEX interest_asset_date_idx ON interest (asset_id, date DESC)
    WHERE asset_id IS NOT NULL;

CREATE OR REPLACE FUNCTION assert_asset_trade_nonnegative(target_asset_id TEXT)
RETURNS VOID AS $$
DECLARE
    minimum_inventory NUMERIC;
BEGIN
    PERFORM 1 FROM asset WHERE id = target_asset_id FOR NO KEY UPDATE;
    SELECT MIN(running_inventory) INTO minimum_inventory
    FROM (
        SELECT SUM(CASE WHEN direction = 'Buy' THEN units ELSE -units END)
            OVER (ORDER BY date, CASE WHEN direction = 'Buy' THEN 0 ELSE 1 END, created_at, id)
            AS running_inventory
        FROM asset_trade
        WHERE asset_id = target_asset_id
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
        PERFORM assert_asset_trade_nonnegative(OLD.asset_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM assert_asset_trade_nonnegative(NEW.asset_id);
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER asset_trade_nonnegative
AFTER INSERT OR UPDATE OR DELETE ON asset_trade
DEFERRABLE INITIALLY IMMEDIATE
FOR EACH ROW EXECUTE FUNCTION validate_asset_trade_ledger();

CREATE OR REPLACE FUNCTION assert_asset_balance_nonnegative(target_asset_id TEXT)
RETURNS VOID AS $$
DECLARE
    current_class TEXT;
    current_balance NUMERIC := 0;
    opening_date TIMESTAMP WITH TIME ZONE;
    entry RECORD;
BEGIN
    SELECT asset_class INTO current_class FROM asset WHERE id = target_asset_id FOR NO KEY UPDATE;
    IF current_class IS NULL OR current_class NOT IN ('CashAccount', 'PrivateDebt') THEN
        RETURN;
    END IF;

    SELECT date INTO opening_date
    FROM asset_balance_snapshot
    WHERE asset_id = target_asset_id AND kind = 'Opening';

    FOR entry IN
        SELECT * FROM (
            SELECT date,
                CASE kind WHEN 'Opening' THEN 0 ELSE 3 END AS event_order,
                created_at,
                id,
                kind,
                balance AS amount
            FROM asset_balance_snapshot
            WHERE asset_id = target_asset_id
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
            WHERE asset_id = target_asset_id
                AND (opening_date IS NULL OR date >= opening_date)
            UNION ALL
            SELECT date, 1 AS event_order, date AS created_at, id, 'Interest' AS kind, amount
            FROM interest
            WHERE asset_id = target_asset_id
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
        PERFORM assert_asset_balance_nonnegative(OLD.asset_id);
    END IF;
    IF TG_OP <> 'DELETE' AND (TG_OP = 'INSERT' OR NEW.asset_id IS DISTINCT FROM OLD.asset_id) THEN
        PERFORM assert_asset_balance_nonnegative(NEW.asset_id);
    ELSIF TG_OP = 'UPDATE' THEN
        PERFORM assert_asset_balance_nonnegative(NEW.asset_id);
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER asset_transaction_nonnegative
AFTER INSERT OR UPDATE OR DELETE ON asset_transaction
DEFERRABLE INITIALLY IMMEDIATE
FOR EACH ROW EXECUTE FUNCTION validate_asset_balance_ledger();

CREATE CONSTRAINT TRIGGER asset_snapshot_nonnegative
AFTER INSERT OR UPDATE OR DELETE ON asset_balance_snapshot
DEFERRABLE INITIALLY IMMEDIATE
FOR EACH ROW EXECUTE FUNCTION validate_asset_balance_ledger();

CREATE CONSTRAINT TRIGGER asset_interest_nonnegative
AFTER INSERT OR UPDATE OF asset_id, date, amount OR DELETE ON interest
DEFERRABLE INITIALLY IMMEDIATE
FOR EACH ROW EXECUTE FUNCTION validate_asset_balance_ledger();

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
    FROM asset WHERE id = NEW.asset_id FOR NO KEY UPDATE;

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

CREATE TRIGGER validate_asset_trade
BEFORE INSERT OR UPDATE ON asset_trade
FOR EACH ROW EXECUTE FUNCTION validate_asset_activity();

CREATE TRIGGER validate_asset_valuation
BEFORE INSERT OR UPDATE ON asset_valuation
FOR EACH ROW EXECUTE FUNCTION validate_asset_activity();

CREATE TRIGGER validate_asset_transaction
BEFORE INSERT OR UPDATE ON asset_transaction
FOR EACH ROW EXECUTE FUNCTION validate_asset_activity();

CREATE TRIGGER validate_asset_snapshot
BEFORE INSERT OR UPDATE ON asset_balance_snapshot
FOR EACH ROW EXECUTE FUNCTION validate_asset_activity();

CREATE TRIGGER validate_asset_interest
BEFORE INSERT OR UPDATE OF asset_id, date, amount, currency ON interest
FOR EACH ROW EXECUTE FUNCTION validate_asset_activity();

CREATE OR REPLACE FUNCTION track_asset_activity()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE asset SET activity_revision = activity_revision + 1 WHERE id = OLD.asset_id;
        RETURN OLD;
    ELSIF TG_OP = 'INSERT' THEN
        UPDATE asset SET activity_revision = activity_revision + 1 WHERE id = NEW.asset_id;
    ELSIF NEW.asset_id IS DISTINCT FROM OLD.asset_id THEN
        UPDATE asset SET activity_revision = activity_revision + 1 WHERE id = OLD.asset_id;
        UPDATE asset SET activity_revision = activity_revision + 1 WHERE id = NEW.asset_id;
    ELSE
        UPDATE asset SET activity_revision = activity_revision + 1 WHERE id = NEW.asset_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER track_asset_trade_activity
AFTER INSERT OR UPDATE OR DELETE ON asset_trade
FOR EACH ROW EXECUTE FUNCTION track_asset_activity();

CREATE TRIGGER track_asset_valuation_activity
AFTER INSERT OR UPDATE OR DELETE ON asset_valuation
FOR EACH ROW EXECUTE FUNCTION track_asset_activity();

CREATE TRIGGER track_asset_transaction_activity
AFTER INSERT OR UPDATE OR DELETE ON asset_transaction
FOR EACH ROW EXECUTE FUNCTION track_asset_activity();

CREATE TRIGGER track_asset_snapshot_activity
AFTER INSERT OR UPDATE OR DELETE ON asset_balance_snapshot
FOR EACH ROW EXECUTE FUNCTION track_asset_activity();

CREATE TRIGGER track_asset_interest_activity
AFTER INSERT OR UPDATE OF asset_id, date, amount, currency OR DELETE ON interest
FOR EACH ROW EXECUTE FUNCTION track_asset_activity();

CREATE OR REPLACE FUNCTION validate_asset_metadata_update()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.asset_class IN ('CashAccount', 'PrivateDebt') AND NEW.unit_label <> NEW.currency THEN
        RAISE EXCEPTION 'cash and private debt unit labels must match their currency';
    END IF;

    IF (NEW.currency IS DISTINCT FROM OLD.currency OR NEW.unit_label IS DISTINCT FROM OLD.unit_label)
        AND (
            EXISTS (SELECT 1 FROM asset_trade WHERE asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM asset_valuation WHERE asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM asset_transaction WHERE asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM asset_balance_snapshot WHERE asset_id = OLD.id)
            OR EXISTS (SELECT 1 FROM interest WHERE asset_id = OLD.id)
        ) THEN
        RAISE EXCEPTION 'asset currency and unit label cannot change after activity is recorded';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER validate_asset_metadata
BEFORE UPDATE OF currency, unit_label ON asset
FOR EACH ROW EXECUTE FUNCTION validate_asset_metadata_update();
