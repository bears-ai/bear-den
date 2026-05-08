CREATE TABLE IF NOT EXISTS bear_web_sources (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('host', 'url')),
    scope_value TEXT NOT NULL,
    label TEXT NULL,
    policy TEXT NOT NULL CHECK (policy IN ('preferred', 'allowed', 'blocked')),
    priority INTEGER NOT NULL DEFAULT 0,
    created_by_user_id INTEGER NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (bear_id, scope_kind, scope_value)
);

CREATE INDEX IF NOT EXISTS idx_bear_web_sources_bear_policy
    ON bear_web_sources (bear_id, policy, scope_kind, scope_value);

CREATE TABLE IF NOT EXISTS bear_web_approvals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('host', 'url')),
    scope_value TEXT NOT NULL,
    approved_by_user_id INTEGER NULL REFERENCES users(id) ON DELETE SET NULL,
    source TEXT NOT NULL DEFAULT 'acp' CHECK (source IN ('acp', 'web', 'admin')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NULL,
    revoked_at TIMESTAMPTZ NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_bear_web_approvals_active_unique
    ON bear_web_approvals (bear_id, scope_kind, scope_value)
    WHERE revoked_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_bear_web_approvals_bear_active
    ON bear_web_approvals (bear_id, scope_kind, scope_value)
    WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS bear_web_fetches (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    session_id TEXT NULL,
    tool_call_id TEXT NULL,
    url TEXT NOT NULL,
    final_url TEXT NULL,
    host TEXT NOT NULL,
    execution_location TEXT NOT NULL CHECK (execution_location IN ('den', 'adapter_local')),
    approval_kind TEXT NOT NULL CHECK (approval_kind IN ('preferred', 'allowed', 'user_url', 'user_host', 'allow_once', 'denied', 'not_required')),
    http_status INTEGER NULL,
    content_type TEXT NULL,
    bytes BIGINT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_bear_web_fetches_bear_fetched_at
    ON bear_web_fetches (bear_id, fetched_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_web_fetches_bear_host
    ON bear_web_fetches (bear_id, host);
