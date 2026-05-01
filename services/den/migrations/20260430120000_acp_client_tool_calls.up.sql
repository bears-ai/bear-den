CREATE TABLE IF NOT EXISTS acp_client_tool_calls (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    bear_slug TEXT NOT NULL,
    acp_session_id TEXT NOT NULL,
    codepool_session_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    request_id UUID NOT NULL,
    call_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    arguments JSONB NOT NULL DEFAULT '{}'::jsonb,
    descriptor JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL,
    result JSONB NULL,
    error JSONB NULL,
    approval_outcome JSONB NULL,
    client_observation JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at TIMESTAMPTZ NULL,
    approved_at TIMESTAMPTZ NULL,
    result_received_at TIMESTAMPTZ NULL,
    forwarded_at TIMESTAMPTZ NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_acp_client_tool_calls_request_call
    ON acp_client_tool_calls (request_id, call_id);

CREATE INDEX IF NOT EXISTS idx_acp_client_tool_calls_active
    ON acp_client_tool_calls (user_id, bear_id, acp_session_id, status);

CREATE INDEX IF NOT EXISTS idx_acp_client_tool_calls_expires_at
    ON acp_client_tool_calls (expires_at);

COMMENT ON TABLE acp_client_tool_calls IS 'Pending and completed ACP client tool relay calls for Den adapter sessions.';
