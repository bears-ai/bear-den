-- ACP pair pre-implementation planning gate.
-- This is distinct from bear_work_plans: it controls permission mode and stores
-- reviewable markdown plan artifacts before mutation.

CREATE TABLE IF NOT EXISTS acp_plan_mode_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    bear_slug TEXT NOT NULL,
    acp_session_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'submitted', 'approved', 'rejected', 'cancelled')),
    reason TEXT NOT NULL DEFAULT '',
    requested_by TEXT NOT NULL DEFAULT 'pair'
        CHECK (requested_by IN ('pair', 'user', 'system')),
    previous_permission_mode TEXT NULL,
    plan_artifact_path TEXT NULL,
    plan_title TEXT NULL,
    plan_body TEXT NULL,
    approval_request_id TEXT NULL,
    approved_by_user_id INTEGER NULL REFERENCES users (id) ON DELETE SET NULL,
    approved_at TIMESTAMPTZ NULL,
    rejected_at TIMESTAMPTZ NULL,
    closed_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (btrim(acp_session_id) <> ''),
    CHECK (btrim(bear_slug) <> ''),
    CHECK (plan_artifact_path IS NULL OR btrim(plan_artifact_path) <> ''),
    CHECK (plan_title IS NULL OR btrim(plan_title) <> ''),
    CHECK (plan_body IS NULL OR btrim(plan_body) <> '')
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_acp_plan_mode_one_open_session
    ON acp_plan_mode_sessions (user_id, bear_id, acp_session_id)
    WHERE state IN ('active', 'submitted');

CREATE INDEX IF NOT EXISTS idx_acp_plan_mode_bear_session
    ON acp_plan_mode_sessions (bear_slug, acp_session_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS acp_plan_mode_events (
    id BIGSERIAL PRIMARY KEY,
    plan_mode_id UUID NOT NULL REFERENCES acp_plan_mode_sessions (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    acp_session_id TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK (event_type IN (
        'enter_requested',
        'entered',
        'artifact_written',
        'exit_requested',
        'approved',
        'rejected',
        'cancelled'
    )),
    event_payload JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (jsonb_typeof(event_payload) = 'object')
);

CREATE INDEX IF NOT EXISTS idx_acp_plan_mode_events_plan_time
    ON acp_plan_mode_events (plan_mode_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_acp_plan_mode_events_bear_time
    ON acp_plan_mode_events (bear_id, created_at DESC);

COMMENT ON TABLE acp_plan_mode_sessions IS 'ACP pair plan-mode gates for read-only planning and user approval before mutation.';
COMMENT ON COLUMN acp_plan_mode_sessions.plan_artifact_path IS 'Durable markdown plan artifact path, e.g. pair/plans/acp-<session>-<id>.md.';
COMMENT ON TABLE acp_plan_mode_events IS 'Append-only audit stream for ACP pair plan-mode state changes.';
