-- Per-bear chat activity for the member-facing bear details page (Den web UI sends).
CREATE TABLE bear_chat_activity (
    id BIGSERIAL PRIMARY KEY,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel TEXT NOT NULL DEFAULT 'den_web',
    message_preview TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_bear_chat_activity_bear_created ON bear_chat_activity (bear_id, created_at DESC);
