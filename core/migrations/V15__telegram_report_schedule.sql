-- A single API-owned daily UTC report schedule, initially paused.
CREATE TABLE telegram_report_schedule (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    send_time_utc TEXT NOT NULL DEFAULT '18:00'
        CHECK (send_time_utc ~ '^([01][0-9]|2[0-3]):[0-5][0-9]$'),
    next_report_at TIMESTAMPTZ,
    last_sent_at TIMESTAMPTZ,
    last_status TEXT NOT NULL DEFAULT 'not_run'
        CHECK (last_status IN ('not_run', 'running', 'succeeded', 'failed')),
    last_error TEXT,
    CHECK (enabled OR next_report_at IS NULL)
);
INSERT INTO telegram_report_schedule(id) VALUES (TRUE);
