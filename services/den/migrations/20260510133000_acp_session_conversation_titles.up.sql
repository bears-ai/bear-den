ALTER TABLE acp_sessions
    ADD COLUMN IF NOT EXISTS conversation_title TEXT,
    ADD COLUMN IF NOT EXISTS conversation_title_updated_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS conversation_title_synced_at TIMESTAMPTZ;
