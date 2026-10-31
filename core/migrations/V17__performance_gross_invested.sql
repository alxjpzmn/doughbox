-- Legacy snapshots retain an unknown gross purchase denominator; never derive it from net capital.
ALTER TABLE performance ADD COLUMN gross_invested NUMERIC;
