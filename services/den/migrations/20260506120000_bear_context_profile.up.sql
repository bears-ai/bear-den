ALTER TABLE bears
    ADD COLUMN IF NOT EXISTS context_profile JSONB NULL;

COMMENT ON COLUMN bears.context_profile IS 'Role-aware context composition profile: role contracts, user steering, Bear context, template metadata, starter prompts.';
