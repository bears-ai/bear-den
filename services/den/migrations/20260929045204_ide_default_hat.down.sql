DROP INDEX bears_ide_default_hat_id_idx;
ALTER TABLE bears DROP CONSTRAINT bears_ide_default_hat_fkey;
ALTER TABLE bears DROP COLUMN ide_default_hat_id;
