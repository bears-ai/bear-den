-- Multi-agent bear registry: role-specific Letta agents, skill manifest, and watch subscriptions.
-- Legacy bears.letta_agent_id remains as a transitional mirror of the talk role.

ALTER TABLE bears
    ADD COLUMN IF NOT EXISTS memfs_repo_path TEXT,
    ADD COLUMN IF NOT EXISTS provisioning_version INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS bear_agents (
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('talk', 'pair', 'curate', 'work', 'watch')),
    letta_agent_id TEXT NULL,
    provisioning_status TEXT NOT NULL DEFAULT 'pending' CHECK (provisioning_status IN ('pending', 'provisioning', 'ready', 'drifted', 'failed')),
    last_provisioned_version INTEGER NOT NULL DEFAULT 0,
    last_synced_at TIMESTAMPTZ NULL,
    last_provisioning_error TEXT NULL,
    config_hash JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (bear_id, role)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_bear_agents_letta_agent_id_unique
    ON bear_agents (letta_agent_id)
    WHERE letta_agent_id IS NOT NULL AND btrim(letta_agent_id) <> '';

CREATE INDEX IF NOT EXISTS idx_bear_agents_role ON bear_agents (role);
CREATE INDEX IF NOT EXISTS idx_bear_agents_provisioning_status ON bear_agents (provisioning_status);

CREATE TABLE IF NOT EXISTS bear_skills_manifest (
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    skill_name TEXT NOT NULL,
    skill_version TEXT NOT NULL,
    source TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    applies_to_roles TEXT[] NOT NULL,
    installed_at TIMESTAMPTZ NULL,
    last_verified_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (bear_id, skill_name, skill_version),
    CHECK (array_length(applies_to_roles, 1) > 0),
    CHECK (applies_to_roles <@ ARRAY['talk', 'pair', 'curate', 'work', 'watch']::TEXT[])
);

CREATE INDEX IF NOT EXISTS idx_bear_skills_manifest_bear_id ON bear_skills_manifest (bear_id);

CREATE TABLE IF NOT EXISTS bear_skill_proposals (
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    proposed_by_agent_id TEXT NOT NULL,
    proposed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    skill_payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending_review' CHECK (status IN ('pending_review', 'approved', 'rejected')),
    reviewed_at TIMESTAMPTZ NULL,
    rejection_reason TEXT NULL,
    resulting_manifest_bear_id UUID NULL,
    resulting_manifest_skill_name TEXT NULL,
    resulting_manifest_skill_version TEXT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (resulting_manifest_bear_id, resulting_manifest_skill_name, resulting_manifest_skill_version)
        REFERENCES bear_skills_manifest (bear_id, skill_name, skill_version)
        ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_bear_skill_proposals_review_queue
    ON bear_skill_proposals (bear_id, status, proposed_at);

CREATE TABLE IF NOT EXISTS bear_subscriptions (
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    subscription_id TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_config JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'failed', 'deleted')),
    approved_task_id TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_fire_at TIMESTAMPTZ NULL,
    error_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bear_id, subscription_id)
);

CREATE INDEX IF NOT EXISTS idx_bear_subscriptions_status ON bear_subscriptions (bear_id, status);

-- Preserve legacy access paths by registering existing single-agent bears as talk-role rows.
INSERT INTO bear_agents (
    bear_id,
    role,
    letta_agent_id,
    provisioning_status,
    last_provisioned_version,
    last_synced_at,
    created_at,
    updated_at
)
SELECT
    id,
    'talk',
    NULLIF(btrim(letta_agent_id), ''),
    CASE WHEN letta_agent_id IS NULL OR btrim(letta_agent_id) = '' THEN 'pending' ELSE 'ready' END,
    provisioning_version,
    CASE WHEN letta_agent_id IS NULL OR btrim(letta_agent_id) = '' THEN NULL ELSE now() END,
    now(),
    now()
FROM bears
ON CONFLICT (bear_id, role) DO NOTHING;

COMMENT ON COLUMN bears.letta_agent_id IS 'Legacy transitional mirror of bear_agents role=talk; new code should use bear_agents.';
COMMENT ON TABLE bear_agents IS 'Role-specific Letta runtime identities for a logical Bear.';
COMMENT ON TABLE bear_skills_manifest IS 'Den-managed canonical skill manifest with per-role applicability.';
COMMENT ON TABLE bear_skill_proposals IS 'Agent-proposed skills awaiting curate review.';
COMMENT ON TABLE bear_subscriptions IS 'Den-owned durable watch subscriptions for inbound external streams.';
