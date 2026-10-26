-- Bootstrap environment defaults in one transaction at runtime, not in the migration.
-- Feed enabled flags retain selection independently of the global scheduler switch.
CREATE TABLE price_scheduler_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    enabled BOOLEAN NOT NULL,
    interval_minutes INTEGER NOT NULL CHECK (interval_minutes BETWEEN 1 AND 1440)
);
