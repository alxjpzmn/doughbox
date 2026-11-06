-- Global checkpoints cannot stand in for a job's pinned portfolio universe.
ALTER TABLE market_data_file ADD COLUMN scope_hash TEXT;
UPDATE market_data_file SET scope_hash = 'global';
ALTER TABLE market_data_file ALTER COLUMN scope_hash SET NOT NULL;
ALTER TABLE market_data_file DROP CONSTRAINT market_data_file_pkey;
ALTER TABLE market_data_file ADD PRIMARY KEY (feed, file_key, parser_version, scope_hash);

-- Books remain request-driven; never seed bid/ask state from last-trade reports.
DROP TABLE mifid_latest_price;
DROP INDEX market_price_report_latest;
