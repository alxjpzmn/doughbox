-- One explicitly selected Trading 212 account; credentials never leave the server.
CREATE TABLE trading_connection (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    generation TEXT NOT NULL,
    environment TEXT CHECK (environment IN ('demo', 'live')),
    credentials TEXT,
    account JSONB,
    instruments JSONB NOT NULL DEFAULT '[]',
    agent_token_hash TEXT,
    CHECK ((environment IS NULL) = (credentials IS NULL))
);
INSERT INTO trading_connection (singleton, generation) VALUES (TRUE, 'unconfigured');

CREATE TABLE trading_proposals (
    id TEXT PRIMARY KEY,
    generation TEXT NOT NULL,
    client_request_id TEXT NOT NULL,
    request JSONB NOT NULL,
    title TEXT NOT NULL,
    note TEXT NOT NULL,
    environment TEXT NOT NULL CHECK (environment IN ('demo', 'live')),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'executing', 'completed', 'discarded')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    trades JSONB NOT NULL,
    UNIQUE (generation, client_request_id)
);
CREATE INDEX trading_proposals_review ON trading_proposals (status, created_at DESC);
