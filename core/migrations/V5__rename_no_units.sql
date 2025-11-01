DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_attribute 
               WHERE attname = 'no_units' 
               AND attrelid = 'trade'::regclass) THEN
        EXECUTE 'ALTER TABLE trade RENAME COLUMN no_units TO units';
    END IF;
END $$;

