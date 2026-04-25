-- Phase 1: per-bear Letta fields (no shared template table — duplicate bear in UI when needed).

-- Operator may create a bear row before Letta returns an agent id.
ALTER TABLE bears
    ALTER COLUMN letta_agent_id DROP NOT NULL;

ALTER TABLE bears
    ADD COLUMN IF NOT EXISTS system_prompt TEXT NOT NULL DEFAULT '';
