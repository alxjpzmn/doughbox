-- Account budgets deliberately cross application-owner boundaries. No plaintext
-- broker credentials or owner-facing cross-user account data is retained here.
CREATE TABLE trading_rate_limit (
    environment TEXT NOT NULL CHECK (environment IN ('demo','live')),
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('account','key')),
    scope_id TEXT NOT NULL CHECK (btrim(scope_id) <> ''),
    bucket TEXT NOT NULL CHECK (bucket IN ('account_summary','positions','instruments','pending_orders','order_history','limit_order','pie_list','pie_request','export_create','export_status')),
    next_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
    blocked_until TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
    interval_millis BIGINT NOT NULL CHECK (interval_millis > 0),
    PRIMARY KEY(environment,scope_kind,scope_id,bucket)
);
CREATE TABLE trading_api_key_account (
    environment TEXT NOT NULL CHECK (environment IN ('demo','live')),
    key_hash TEXT NOT NULL,
    broker_account_id TEXT NOT NULL CHECK (btrim(broker_account_id) <> ''),
    PRIMARY KEY(environment,key_hash)
);

-- Retain the greatest outstanding cooldown for every duplicate-account
-- connection before retiring owner-local pacing. Old request times were claims,
-- whereas *_next_at columns already contain their complete deadline.
INSERT INTO trading_rate_limit(environment,scope_kind,scope_id,bucket,next_at,blocked_until,interval_millis)
SELECT c.environment,'account',c.account->>'id',b.bucket,max(b.deadline),'epoch',b.spacing
FROM trading_connection c CROSS JOIN LATERAL (VALUES
    ('limit_order',c.last_submission_at + interval '3 seconds',3000::bigint),
    ('pending_orders',c.last_pending_poll_at + interval '6 seconds',6000::bigint),
    ('order_history',c.last_history_poll_at + interval '6 seconds',6000::bigint),
    ('instruments',c.last_instruments_request_at + interval '60 seconds',60000::bigint),
    ('pie_list',c.pie_list_next_at,31000::bigint),
    ('pie_request',c.pie_request_next_at,6000::bigint),
    ('account_summary',c.account_next_at,6000::bigint),
    ('positions',c.positions_next_at,2000::bigint)
) b(bucket,deadline,spacing)
WHERE c.credentials IS NOT NULL AND c.environment IS NOT NULL
    AND jsonb_typeof(c.account->'id')='string' AND btrim(c.account->>'id')<>''
GROUP BY c.environment,c.account->>'id',b.bucket,b.spacing;

ALTER TABLE trading_connection
    DROP COLUMN last_submission_at,
    DROP COLUMN last_pending_poll_at,
    DROP COLUMN last_history_poll_at,
    DROP COLUMN last_instruments_request_at,
    DROP COLUMN pie_list_next_at,
    DROP COLUMN pie_request_next_at,
    DROP COLUMN account_next_at,
    DROP COLUMN positions_next_at;
