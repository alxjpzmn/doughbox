CREATE TABLE IF NOT EXISTS tax_optimizations (
    id TEXT PRIMARY KEY,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    broker TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    currency TEXT NOT NULL,
    amount_eur NUMERIC NOT NULL,
    tax_type TEXT NOT NULL,
    description TEXT,
    transaction_id TEXT
);
