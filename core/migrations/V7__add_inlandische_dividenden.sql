ALTER TABLE fund_report_oekb ADD COLUMN IF NOT EXISTS inlaendische_dividenden NUMERIC NOT NULL DEFAULT 0.0;
ALTER TABLE fund_report_oekb ADD COLUMN IF NOT EXISTS kest_inlaendische_dividenden NUMERIC NOT NULL DEFAULT 0.0;
