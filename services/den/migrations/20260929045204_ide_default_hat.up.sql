-- An IDE default is an explicit Bear-owned preference, not an implicit
-- conversation or Job binding. Existing Bears remain unconfigured.
ALTER TABLE bears ADD COLUMN ide_default_hat_id UUID NULL;
ALTER TABLE bears ADD CONSTRAINT bears_ide_default_hat_fkey
    FOREIGN KEY (id, ide_default_hat_id) REFERENCES bear_hats (bear_id, id) ON DELETE RESTRICT;
CREATE INDEX bears_ide_default_hat_id_idx ON bears (ide_default_hat_id)
    WHERE ide_default_hat_id IS NOT NULL;
