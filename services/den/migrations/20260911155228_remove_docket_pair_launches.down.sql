CREATE TABLE docket_pair_launches (
    run_id TEXT PRIMARY KEY REFERENCES turn_runs (run_id) ON DELETE CASCADE,
    attempt_id UUID NOT NULL REFERENCES docket_execution_attempts (id) ON DELETE CASCADE,
    fence_epoch BIGINT NOT NULL CHECK (fence_epoch > 0),
    state TEXT NOT NULL CHECK (state IN ('queued', 'claimed', 'started', 'failed', 'cancelled')),
    claim_id UUID NULL,
    claim_expires_at TIMESTAMPTZ NULL,
    started_at TIMESTAMPTZ NULL,
    terminal_reason TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((claim_id IS NULL) = (claim_expires_at IS NULL)),
    CHECK (state <> 'claimed' OR claim_id IS NOT NULL),
    CHECK (state <> 'started' OR started_at IS NOT NULL),
    UNIQUE (attempt_id, fence_epoch)
);

CREATE INDEX docket_pair_launches_recoverable_idx
    ON docket_pair_launches (state, claim_expires_at, created_at)
    WHERE state IN ('queued', 'claimed');

COMMENT ON TABLE docket_pair_launches IS
    'Legacy durable launch ownership restored for rollback compatibility.';
