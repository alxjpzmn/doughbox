-- Older aggregate snapshots cannot be decomposed reliably or retroactively gain income.
ALTER TABLE performance ADD COLUMN components JSONB;
