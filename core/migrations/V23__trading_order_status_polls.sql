-- Trading 212 pending orders are limited to 1 request / 5s.
-- Historical orders, which confirm fills, are limited to 20 requests / minute.
ALTER TABLE trading_connection
    ADD COLUMN last_pending_poll_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
    ADD COLUMN last_history_poll_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch';
