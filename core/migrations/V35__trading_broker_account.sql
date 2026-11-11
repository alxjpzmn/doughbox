ALTER TABLE trading_connection
    ADD COLUMN account_next_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
    ADD COLUMN positions_next_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch';

ALTER TABLE trading_proposals
    ADD COLUMN broker_account JSONB,
    ADD CONSTRAINT trading_proposals_broker_account CHECK (
        broker_account IS NULL OR (kind = 'trades' AND jsonb_typeof(broker_account) = 'object')
    );
