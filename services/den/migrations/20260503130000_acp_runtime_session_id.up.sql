-- ACP is API-direct to the pair role now; rename historical Codepool-specific session bindings.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'acp_sessions'
          AND column_name = 'codepool_session_id'
    ) AND NOT EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'acp_sessions'
          AND column_name = 'runtime_session_id'
    ) THEN
        ALTER TABLE acp_sessions
            RENAME COLUMN codepool_session_id TO runtime_session_id;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'acp_client_tool_calls'
          AND column_name = 'codepool_session_id'
    ) AND NOT EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'acp_client_tool_calls'
          AND column_name = 'runtime_session_id'
    ) THEN
        ALTER TABLE acp_client_tool_calls
            RENAME COLUMN codepool_session_id TO runtime_session_id;
    END IF;
END $$;

COMMENT ON TABLE acp_sessions IS 'ACP client session bindings mapped to pair-role Letta conversations for lifecycle handling.';
COMMENT ON COLUMN acp_sessions.runtime_session_id IS 'Runtime-neutral ACP session binding id. Historical deployments called this codepool_session_id.';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'acp_sessions'
          AND column_name = 'adapter_environment'
    ) THEN
        ALTER TABLE acp_sessions
            ADD COLUMN adapter_environment JSONB NULL;
    END IF;
END $$;

COMMENT ON COLUMN acp_sessions.adapter_environment IS 'Latest adapter-published environment snapshot for this ACP session. This is a BearWire-like runtime report owned by the trusted adapter/edge process.';
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_name = 'acp_client_tool_calls'
    ) THEN
        COMMENT ON TABLE acp_client_tool_calls IS 'Pending and completed ACP client tool relay calls for Den adapter sessions.';
    END IF;
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'acp_client_tool_calls'
          AND column_name = 'runtime_session_id'
    ) THEN
        COMMENT ON COLUMN acp_client_tool_calls.runtime_session_id IS 'Runtime-neutral ACP session binding id. Historical deployments called this codepool_session_id.';
    END IF;
END $$;
