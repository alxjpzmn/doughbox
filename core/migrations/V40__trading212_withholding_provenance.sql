-- Unknown history-list income withholding is not a reported zero. Keep the
-- marker on the ledger identity so accepted corrections cannot redefine it.
ALTER TABLE t212_activity_event
    ADD COLUMN withholding_tax_reported BOOLEAN NOT NULL DEFAULT true,
    ADD CONSTRAINT t212_unknown_withholding_income_only
        CHECK (withholding_tax_reported OR kind IN ('dividend', 'interest'));
