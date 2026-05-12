CREATE TABLE IF NOT EXISTS bear_reflection_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    lane TEXT NOT NULL,
    trigger TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN (
        'queued',
        'running',
        'completed',
        'failed',
        'cancelled',
        'skipped',
        'needs_human_review'
    )),
    role_agent_id TEXT NULL,
    conversation_id TEXT NULL,
    conversation_key TEXT NULL,
    conversation_date DATE NULL,
    input_summary JSONB NOT NULL DEFAULT '{}',
    output_summary JSONB NOT NULL DEFAULT '{}',
    error TEXT NULL,
    started_at TIMESTAMPTZ NULL,
    completed_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_bear_reflection_runs_bear_lane_status_created
    ON bear_reflection_runs (bear_id, lane, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_reflection_runs_bear_conversation_date
    ON bear_reflection_runs (bear_id, lane, conversation_date DESC, created_at DESC);

CREATE TABLE IF NOT EXISTS bear_reflection_run_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES bear_reflection_runs(id) ON DELETE CASCADE,
    item_kind TEXT NOT NULL,
    item_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_bear_reflection_run_items_run_created
    ON bear_reflection_run_items (run_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_reflection_run_items_kind_item
    ON bear_reflection_run_items (item_kind, item_id);

CREATE TABLE IF NOT EXISTS reflection_conversations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    role_agent_id TEXT NULL,
    lane TEXT NOT NULL,
    conversation_date DATE NOT NULL,
    conversation_key TEXT NOT NULL,
    conversation_id TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_reflection_conversations_bear_lane_date UNIQUE (bear_id, lane, conversation_date)
);

CREATE INDEX IF NOT EXISTS idx_reflection_conversations_bear_lane_used
    ON reflection_conversations (bear_id, lane, last_used_at DESC);
