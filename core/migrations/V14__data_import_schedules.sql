-- API/manual and scheduled imports share status; CLI housekeeping and implicit FX fetches do not.
ALTER TABLE data_import_status
    ADD COLUMN schedule_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN schedule_frequency TEXT NOT NULL DEFAULT 'daily'
        CHECK (schedule_frequency IN ('daily', 'weekly', 'monthly')),
    ADD COLUMN next_refresh_at TIMESTAMPTZ,
    ADD CONSTRAINT data_import_schedule_next_refresh
        CHECK (schedule_enabled = (next_refresh_at IS NOT NULL));

UPDATE data_import_status SET schedule_frequency = 'monthly' WHERE source = 'polygon';

COMMENT ON TABLE data_import_status IS
    'Persisted API/manual and scheduled reference-data imports; excludes CLI housekeeping and implicit FX fetches';
COMMENT ON COLUMN data_import_status.next_refresh_at IS
    'Next UTC refresh; enabling or changing an enabled cadence makes it due immediately; NULL while paused';
