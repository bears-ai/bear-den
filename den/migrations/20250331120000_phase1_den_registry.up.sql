-- Phase 1 M1: Den bear registry + membership (schema only; no application logic in this milestone).
-- Legacy `users.id` remains INTEGER until a later migration moves identity to UUID; `user_bear.user_id`
-- follows that type so FKs are valid. Bear identifiers use UUID as in PHASE1_BOOTSTRAP.md.

-- Optional external chat client id (nullable; legacy column name — BEARS uses Den embedded chat only).
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS webui_account_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS users_webui_account_id_key ON users (webui_account_id)
    WHERE webui_account_id IS NOT NULL;

-- Operator flag (PHASE1 name); keep `admin_flag` for existing app code until auth handlers are switched.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT false;

UPDATE users
SET is_admin = COALESCE(admin_flag, false)
WHERE is_admin IS DISTINCT FROM COALESCE(admin_flag, false);

CREATE TABLE bears (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid (),
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    letta_agent_id TEXT NOT NULL,
    default_model TEXT NULL,
    tools_enabled JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE user_bear (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    role TEXT NULL,
    PRIMARY KEY (user_id, bear_id)
);

CREATE INDEX IF NOT EXISTS idx_user_bear_bear_id ON user_bear (bear_id);

CREATE TABLE audit_chat (
    id BIGSERIAL PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    bytes_out INT NULL
);
