-- User-owned ACP access tokens with explicit per-bear grants.
CREATE TABLE IF NOT EXISTS acp_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes JSONB NOT NULL DEFAULT '["acp:chat"]'::jsonb,
    expires_at TIMESTAMPTZ NULL,
    last_used_at TIMESTAMPTZ NULL,
    revoked_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_acp_tokens_user_id ON acp_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_acp_tokens_active_user ON acp_tokens (user_id, revoked_at, expires_at);

CREATE TABLE IF NOT EXISTS acp_token_bears (
    token_id UUID NOT NULL REFERENCES acp_tokens (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (token_id, bear_id)
);

CREATE INDEX IF NOT EXISTS idx_acp_token_bears_bear_id ON acp_token_bears (bear_id);

COMMENT ON TABLE acp_tokens IS 'User-owned personal access tokens for ACP adapters. Store only token hashes; raw tokens are shown once.';
COMMENT ON TABLE acp_token_bears IS 'Per-bear grants for ACP tokens. User membership is still checked at request time.';
