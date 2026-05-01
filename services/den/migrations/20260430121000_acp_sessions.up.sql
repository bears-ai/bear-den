CREATE TABLE IF NOT EXISTS acp_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    bear_slug TEXT NOT NULL,
    acp_session_id TEXT NOT NULL,
    codepool_session_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    resolved_conversation_id TEXT NULL,
    client TEXT NOT NULL,
    cwd TEXT NULL,
    closed_at TIMESTAMPTZ NULL,
    archived_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, bear_id, acp_session_id)
);

CREATE INDEX IF NOT EXISTS idx_acp_sessions_bear_session
    ON acp_sessions (bear_slug, acp_session_id);

CREATE INDEX IF NOT EXISTS idx_acp_sessions_conversation_id
    ON acp_sessions (conversation_id);

COMMENT ON TABLE acp_sessions IS 'ACP client sessions mapped to Codepool/Letta conversations for lifecycle handling.';
