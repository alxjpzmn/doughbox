-- CSV export is not offered on every account that has history access. Those
-- runs page the history list endpoints instead of failing as a missing export scope.
DO $$
DECLARE constraint_name text;
BEGIN
    SELECT con.conname INTO constraint_name
    FROM pg_constraint con
    JOIN pg_class rel ON rel.oid = con.conrelid
    WHERE rel.relname = 't212_sync_run' AND con.contype = 'c'
      AND pg_get_constraintdef(con.oid) LIKE '%request_report%';
    IF constraint_name IS NULL THEN
        RAISE EXCEPTION 't212_sync_run phase check was not found';
    END IF;
    EXECUTE format('ALTER TABLE t212_sync_run DROP CONSTRAINT %I', constraint_name);
END $$;
ALTER TABLE t212_sync_run ADD CONSTRAINT t212_sync_run_phase_check
    CHECK (phase IN (
        'request_report', 'poll_report', 'download_report', 'import_report',
        'history_orders', 'history_dividends', 'history_transactions'
    ));
