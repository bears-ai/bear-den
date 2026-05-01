CREATE TABLE IF NOT EXISTS archived_conversations (
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL,
    archived_by_user_id INTEGER NULL REFERENCES users (id) ON DELETE SET NULL,
    source TEXT NOT NULL DEFAULT 'den',
    archived_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (bear_id, conversation_id)
);

CREATE INDEX IF NOT EXISTS idx_archived_conversations_bear_id
    ON archived_conversations (bear_id);

COMMENT ON TABLE archived_conversations IS 'Den-side archived Letta conversations used when the Letta conversation list does not expose archive state.';
