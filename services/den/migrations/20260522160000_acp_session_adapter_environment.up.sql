-- Store latest adapter-published ACP adapter environment snapshots on ACP sessions.
ALTER TABLE acp_sessions
    ADD COLUMN IF NOT EXISTS adapter_environment JSONB NULL;

COMMENT ON COLUMN acp_sessions.adapter_environment IS 'Latest adapter-published environment snapshot for this ACP session. This is a BearWire-like runtime report owned by the trusted adapter/edge process.';
