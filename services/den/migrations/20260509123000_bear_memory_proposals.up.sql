CREATE TABLE IF NOT EXISTS bear_memory_proposals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    source_role TEXT NOT NULL CHECK (source_role IN ('talk', 'pair', 'curate', 'work', 'watch')),
    source_agent_id TEXT NULL,
    source_paths TEXT[] NOT NULL DEFAULT '{}',
    source_refs JSONB NOT NULL DEFAULT '[]',
    proposal_type TEXT NOT NULL DEFAULT 'memory_review',
    suggested_action TEXT NOT NULL DEFAULT 'unspecified',
    target_ref TEXT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    rationale TEXT NOT NULL DEFAULT '',
    proposed_content TEXT NULL,
    proposed_patch TEXT NULL,
    refs JSONB NOT NULL DEFAULT '{}',
    sensitivity TEXT NOT NULL DEFAULT 'normal' CHECK (sensitivity IN ('normal', 'person', 'secret_risk', 'external_untrusted', 'unknown')),
    requires_human BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'in_review', 'approved', 'rejected', 'retained_local', 'deferred', 'superseded', 'needs_human_review')),
    reviewer_role TEXT NULL CHECK (reviewer_role IS NULL OR reviewer_role IN ('talk', 'pair', 'curate', 'work', 'watch')),
    reviewer_agent_id TEXT NULL,
    review_notes TEXT NULL,
    decision_summary TEXT NULL,
    result_path TEXT NULL,
    result_commit TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    reviewed_at TIMESTAMPTZ NULL
);

CREATE INDEX IF NOT EXISTS idx_bear_memory_proposals_bear_status_created
    ON bear_memory_proposals (bear_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_bear_memory_proposals_bear_source_role_created
    ON bear_memory_proposals (bear_id, source_role, created_at DESC);
