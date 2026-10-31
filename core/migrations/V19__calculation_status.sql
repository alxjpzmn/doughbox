-- Only explicit API refreshes update this metadata; reads never run calculations.
CREATE TABLE calculation_status (
    source TEXT PRIMARY KEY CHECK (source IN ('performance', 'taxation')),
    status TEXT NOT NULL DEFAULT 'never_run'
        CHECK (status IN ('never_run', 'running', 'succeeded', 'failed')),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    last_error TEXT,
    from_date DATE,
    until_date DATE,
    CHECK (source = 'taxation' OR (from_date IS NULL AND until_date IS NULL)),
    CHECK (from_date IS NULL OR until_date IS NULL OR from_date <= until_date)
);

INSERT INTO calculation_status(source) VALUES ('performance'), ('taxation');
