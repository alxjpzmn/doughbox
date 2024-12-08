CREATE TABLE IF NOT EXISTS trades (
    hash TEXT NOT NULL UNIQUE,
    broker TEXT NOT NULL,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    no_units NUMERIC NOT NULL,
    avg_price_per_unit NUMERIC NOT NULL,
    eur_avg_price_per_unit NUMERIC NOT NULL,
    security_type TEXT NOT NULL,
    direction TEXT NOT NULL,
    currency_denomination TEXT NOT NULL,
    isin TEXT NOT NULL,
    date_added TIMESTAMP WITH TIME ZONE NOT NULL,
    fees NUMERIC NOT NULL DEFAULT 0.0,
    withholding_tax NUMERIC,
    witholding_tax_currency TEXT
);

CREATE TABLE IF NOT EXISTS fund_reports (
    id INTEGER NOT NULL UNIQUE,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    isin TEXT NOT NULL,
    currency TEXT NOT NULL,
    dividend NUMERIC NOT NULL DEFAULT 0.0,
    dividend_aequivalent NUMERIC NOT NULL DEFAULT 0.0,
    withheld_dividend NUMERIC NOT NULL DEFAULT 0.0,
    intermittent_dividend NUMERIC NOT NULL DEFAULT 0.0,
    wac_adjustment NUMERIC NOT NULL DEFAULT 0.0
);

CREATE TABLE IF NOT EXISTS fx_rates (
    hash TEXT NOT NULL UNIQUE,
    date DATE NOT NULL,
    rate NUMERIC,
    currency_from TEXT NOT NULL,
    currency_to TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS stock_splits (
    id TEXT NOT NULL UNIQUE,
    ex_date TIMESTAMP WITH TIME ZONE NOT NULL,
    from_factor NUMERIC NOT NULL,
    to_factor NUMERIC NOT NULL,
    isin TEXT NOT NULL,
    date_added TIMESTAMP WITH TIME ZONE NOT NULL
);

CREATE TABLE IF NOT EXISTS fx_conversions (
    id TEXT NOT NULL UNIQUE,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    broker TEXT NOT NULL,
    from_amount NUMERIC NOT NULL,
    to_amount NUMERIC NOT NULL,
    from_currency TEXT NOT NULL,
    to_currency TEXT NOT NULL,
    date_added TIMESTAMP WITH TIME ZONE NOT NULL,
    fees NUMERIC,
    withholding_tax NUMERIC,
    witholding_tax_currency TEXT
);

CREATE TABLE IF NOT EXISTS listing_changes (
    id TEXT NOT NULL UNIQUE,
    ex_date TIMESTAMP WITH TIME ZONE NOT NULL,
    from_factor NUMERIC NOT NULL,
    to_factor NUMERIC NOT NULL,
    from_identifier TEXT NOT NULL,
    to_identifier TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS instruments (
    id TEXT NOT NULL UNIQUE,
    last_price_update TIMESTAMP WITH TIME ZONE NOT NULL,
    price NUMERIC NOT NULL,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS performance (
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    total_value NUMERIC NOT NULL,
    total_invested NUMERIC NOT NULL
);

CREATE TABLE IF NOT EXISTS dividends (
    id TEXT PRIMARY KEY,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    isin TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    broker TEXT,
    currency TEXT,
    amount_eur NUMERIC NOT NULL,
    withholding_tax NUMERIC,
    witholding_tax_currency TEXT
);

CREATE TABLE IF NOT EXISTS interest (
    id TEXT PRIMARY KEY,
    date TIMESTAMP WITH TIME ZONE NOT NULL,
    amount NUMERIC NOT NULL,
    broker TEXT,
    principal TEXT,
    currency TEXT NOT NULL,
    amount_eur NUMERIC NOT NULL,
    withholding_tax NUMERIC,
    witholding_tax_currency TEXT
);

CREATE TABLE IF NOT EXISTS ticker_conversions (
    id UUID DEFAULT gen_random_uuid() PRIMARY KEY,
    ticker TEXT NOT NULL,
    isin TEXT NOT NULL
); 
