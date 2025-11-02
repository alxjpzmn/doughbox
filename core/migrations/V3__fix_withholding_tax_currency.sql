DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_attribute 
               WHERE attname = 'witholding_tax_currency' 
               AND attrelid = 'trade'::regclass) THEN
        EXECUTE 'ALTER TABLE trade RENAME COLUMN witholding_tax_currency TO withholding_tax_currency';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_attribute 
               WHERE attname = 'witholding_tax_currency' 
               AND attrelid = 'interest'::regclass) THEN
        EXECUTE 'ALTER TABLE interest RENAME COLUMN witholding_tax_currency TO withholding_tax_currency';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_attribute 
               WHERE attname = 'witholding_tax_currency' 
               AND attrelid = 'dividend'::regclass) THEN
        EXECUTE 'ALTER TABLE dividend RENAME COLUMN witholding_tax_currency TO withholding_tax_currency';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_attribute 
               WHERE attname = 'witholding_tax_currency' 
               AND attrelid = 'fx_conversion'::regclass) THEN
        EXECUTE 'ALTER TABLE fx_conversion RENAME COLUMN witholding_tax_currency TO withholding_tax_currency';
    END IF;
END $$;
