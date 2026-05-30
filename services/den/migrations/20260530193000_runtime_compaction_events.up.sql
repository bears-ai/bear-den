CREATE TABLE IF NOT EXISTS runtime_compaction_events (
    id BIGSERIAL PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    trigger TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('Applied', 'Skipped', 'Failed')),
    event_hash TEXT NOT NULL,
    boundary JSONB NULL,
    source_group_start INTEGER NULL,
    source_group_end INTEGER NULL,
    artifact JSONB NULL,
    diagnostic TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (boundary IS NULL OR jsonb_typeof(boundary) = 'object'),
    CHECK (artifact IS NULL OR jsonb_typeof(artifact) = 'object')
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_compaction_events_dedupe
    ON runtime_compaction_events (conversation_id, event_hash);

CREATE INDEX IF NOT EXISTS idx_runtime_compaction_events_conversation_time
    ON runtime_compaction_events (conversation_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_runtime_compaction_events_status_time
    ON runtime_compaction_events (status, created_at DESC);

COMMENT ON TABLE runtime_compaction_events IS 'Append-only audit/event stream for Den runtime compaction evaluation and outputs.';
