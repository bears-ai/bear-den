CREATE TABLE IF NOT EXISTS pair_reflection_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    acp_session_id TEXT NOT NULL,
    conversation_id TEXT NULL,
    trigger TEXT NOT NULL DEFAULT 'manual',
    status TEXT NOT NULL DEFAULT 'started' CHECK (status IN ('started', 'completed', 'failed', 'skipped')),
    summary_path TEXT NULL,
    summary_commit TEXT NULL,
    considered_message_count INTEGER NOT NULL DEFAULT 0,
    considered_memory_paths TEXT[] NOT NULL DEFAULT '{}',
    diagnostic JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ NULL
);

CREATE INDEX IF NOT EXISTS idx_pair_reflection_runs_bear_created
    ON pair_reflection_runs (bear_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_pair_reflection_runs_session
    ON pair_reflection_runs (bear_id, user_id, acp_session_id, created_at DESC);
