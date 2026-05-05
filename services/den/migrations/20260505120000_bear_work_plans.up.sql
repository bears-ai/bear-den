-- Den-owned live workboard for per-bear plans and task handoff state.
-- Durable executable tasks still live in MemFS (`talk/tasks`, `pair/tasks`, `core/tasks`, `work/results`).

CREATE TABLE IF NOT EXISTS bear_work_plans (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    owner_role TEXT NOT NULL CHECK (owner_role IN ('talk', 'pair', 'curate', 'work', 'watch')),
    owner_agent_id TEXT NULL,
    created_by_user_id INTEGER NULL REFERENCES users (id) ON DELETE SET NULL,
    source_conversation_id TEXT NULL,
    source_acp_session_id TEXT NULL,
    source_channel JSONB NOT NULL DEFAULT '{}'::JSONB,
    workspace_context JSONB NOT NULL DEFAULT '{}'::JSONB,
    visibility TEXT NOT NULL DEFAULT 'private_to_role'
        CHECK (visibility IN ('private_to_role', 'same_user', 'bear_visible', 'handoff_requested')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'blocked', 'completed', 'cancelled', 'archived')),
    items JSONB NOT NULL DEFAULT '[]'::JSONB,
    version INTEGER NOT NULL DEFAULT 1,
    handoff_intent_path TEXT NULL,
    handoff_task_id TEXT NULL,
    archived_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (btrim(title) <> ''),
    CHECK (jsonb_typeof(items) = 'array'),
    CHECK (jsonb_typeof(source_channel) = 'object'),
    CHECK (jsonb_typeof(workspace_context) = 'object'),
    CHECK (version > 0)
);

CREATE INDEX IF NOT EXISTS idx_bear_work_plans_bear_status
    ON bear_work_plans (bear_id, status, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_work_plans_owner
    ON bear_work_plans (bear_id, owner_role, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_work_plans_source_acp_session
    ON bear_work_plans (bear_id, source_acp_session_id)
    WHERE source_acp_session_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_bear_work_plans_source_conversation
    ON bear_work_plans (bear_id, source_conversation_id)
    WHERE source_conversation_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS bear_work_plan_events (
    id BIGSERIAL PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES bear_work_plans (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    actor_role TEXT NULL CHECK (actor_role IS NULL OR actor_role IN ('talk', 'pair', 'curate', 'work', 'watch')),
    actor_agent_id TEXT NULL,
    actor_user_id INTEGER NULL REFERENCES users (id) ON DELETE SET NULL,
    event_type TEXT NOT NULL CHECK (event_type IN (
        'created',
        'updated',
        'status_changed',
        'visibility_changed',
        'handoff_requested',
        'handoff_linked',
        'archived'
    )),
    event_payload JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (jsonb_typeof(event_payload) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_bear_work_plan_events_plan_time
    ON bear_work_plan_events (plan_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_work_plan_events_bear_time
    ON bear_work_plan_events (bear_id, created_at DESC);

COMMENT ON TABLE bear_work_plans IS 'Den-owned live per-bear workboard for current plans and status; not the durable task execution source.';
COMMENT ON COLUMN bear_work_plans.items IS 'JSON array of plan items; application validation enforces at most one in_progress item.';
COMMENT ON COLUMN bear_work_plans.handoff_intent_path IS 'MemFS path such as pair/tasks/intent-YYYY-MM-DD-001.md after a handoff request is materialized.';
COMMENT ON COLUMN bear_work_plans.handoff_task_id IS 'Approved core task id after curate promotes the handoff intent.';
COMMENT ON TABLE bear_work_plan_events IS 'Append-only audit stream for live workboard changes.';
