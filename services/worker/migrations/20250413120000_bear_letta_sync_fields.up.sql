-- Bear registry: Letta agent type and tool ids for create/patch sync (operator UI).

ALTER TABLE bears
    ADD COLUMN IF NOT EXISTS letta_agent_type TEXT NULL;

ALTER TABLE bears
    ADD COLUMN IF NOT EXISTS letta_tool_ids JSONB NOT NULL DEFAULT '[]'::jsonb;
