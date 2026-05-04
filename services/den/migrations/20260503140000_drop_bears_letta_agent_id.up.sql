-- Runtime Letta ids now live exclusively in bear_agents(role, letta_agent_id).
-- This migration intentionally drops the legacy single-agent mirror from bears.
ALTER TABLE bears DROP COLUMN IF EXISTS letta_agent_id;
