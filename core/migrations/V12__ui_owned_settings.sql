ALTER TABLE price_scheduler_settings
    ADD COLUMN worker_concurrency INTEGER NOT NULL DEFAULT 2
        CHECK (worker_concurrency BETWEEN 1 AND 4);

CREATE TABLE application_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    log_level TEXT NOT NULL DEFAULT 'INFO'
        CHECK (log_level IN ('ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'))
);

INSERT INTO application_settings(singleton) VALUES(TRUE);
