-- Preserve ISIN mappings without replacing the broker connection or agent token.
ALTER TABLE trading_connection
    ADD COLUMN last_instruments_request_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch';

-- Quote requests extend feed coverage without creating portfolio holdings.
CREATE TABLE trading_price_watchlist (
    isin TEXT PRIMARY KEY
);
