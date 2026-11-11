ALTER TABLE trading_proposals
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'trades',
    ADD COLUMN pie JSONB,
    ADD CONSTRAINT trading_proposals_payload_kind CHECK (
        (kind = 'trades' AND pie IS NULL)
        OR (kind = 'pie' AND pie IS NOT NULL AND jsonb_typeof(pie) = 'object' AND trades = '[]'::jsonb)
    );

ALTER TABLE trading_connection
    ADD COLUMN pie_list_next_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
    ADD COLUMN pie_request_next_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch';
