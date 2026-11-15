-- Demo and live are separate broker environments. A verified connection in either
-- environment can import that environment's activity.
ALTER TABLE t212_sync_account DROP CONSTRAINT t212_sync_account_environment_check;
ALTER TABLE t212_sync_account ADD CONSTRAINT t212_sync_account_environment_check
    CHECK (environment IN ('demo', 'live'));

-- Existing verified connections can sync without reconnecting. Live rows already
-- inserted by V36 are left unchanged.
INSERT INTO t212_sync_account(user_id, id, environment, broker_account_id, currency)
SELECT user_id, gen_random_uuid()::text, environment, account->>'id', COALESCE(account->>'currency', '')
FROM trading_connection
WHERE environment IN ('demo', 'live')
    AND credentials IS NOT NULL
    AND jsonb_typeof(account->'id') = 'string'
    AND btrim(account->>'id') <> ''
ON CONFLICT (user_id, environment, broker_account_id) DO NOTHING;
