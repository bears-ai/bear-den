-- Explicit opt-in before a model may distill a private source note into
-- knowledge shared across Bear members. Work-on hats are still denied.
ALTER TABLE bear_hats ADD COLUMN auto_curate_enabled BOOLEAN NOT NULL DEFAULT FALSE;
