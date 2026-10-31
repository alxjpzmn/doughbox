-- Manual calculations and the API-owned scheduler share status and source locks.
ALTER TABLE calculation_status
    ADD COLUMN schedule_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN schedule_frequency TEXT NOT NULL DEFAULT 'monthly'
        CHECK (schedule_frequency IN ('daily', 'weekly', 'monthly')),
    ADD COLUMN next_refresh_at TIMESTAMPTZ,
    ADD CONSTRAINT calculation_schedule_next_refresh
        CHECK (schedule_enabled = (next_refresh_at IS NOT NULL));

COMMENT ON TABLE calculation_status IS
    'Persisted explicit manual and opt-in scheduled calculations; reads never calculate';
COMMENT ON COLUMN calculation_status.next_refresh_at IS
    'Next UTC calculation; enabling or changing an enabled cadence makes it due immediately; NULL while paused';
