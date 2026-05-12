-- Den-mediated ACP session mode. This is the authoritative source for
-- Ask/Plan/Write UI state, while plan-mode gates can further restrict mutation.

ALTER TABLE acp_sessions
    ADD COLUMN IF NOT EXISTS current_mode TEXT NOT NULL DEFAULT 'ask'
        CHECK (current_mode IN ('ask', 'plan', 'write'));

COMMENT ON COLUMN acp_sessions.current_mode IS 'Den-mediated ACP session mode: ask, plan, or write. Client requests are validated by Den before this changes.';
