DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_attribute 
               WHERE attname = 'currency_denomination' 
               AND attrelid = 'trade'::regclass) THEN
        EXECUTE 'ALTER TABLE trade RENAME COLUMN currency_denomination TO currency';
    END IF;
END $$;
