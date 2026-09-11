DROP INDEX IF EXISTS docket_execution_attempts_live_pair_owner_idx;
DROP INDEX IF EXISTS docket_execution_attempts_live_work_owner_idx;

ALTER TABLE docket_execution_attempts
    DROP CONSTRAINT IF EXISTS docket_execution_attempts_work_run_id_fkey,
    DROP COLUMN work_run_id;

ALTER TABLE docket_execution_attempts
    ADD COLUMN owner_kind TEXT,
    ADD COLUMN pair_session_id TEXT,
    ADD COLUMN pair_run_id TEXT,
    ADD COLUMN work_run_id UUID;

UPDATE docket_execution_attempts
SET owner_kind = CASE binding_kind
        WHEN 'client_session' THEN 'pair'
        WHEN 'work_assignment' THEN 'work'
    END,
    pair_session_id = CASE WHEN binding_kind = 'client_session' THEN binding_id END,
    pair_run_id = CASE WHEN host_kind = 'pair' THEN host_run_id END,
    work_run_id = CASE WHEN binding_kind = 'work_assignment' THEN binding_id::uuid END;

ALTER TABLE docket_execution_attempts
    ALTER COLUMN owner_kind SET NOT NULL,
    ADD CONSTRAINT docket_execution_attempts_owner_kind_check
        CHECK (owner_kind IN ('pair', 'work')),
    ADD CONSTRAINT docket_execution_attempts_check CHECK (
        (owner_kind = 'pair' AND pair_session_id IS NOT NULL
         AND btrim(pair_session_id) <> '' AND pair_run_id IS NOT NULL
         AND work_run_id IS NULL)
        OR
        (owner_kind = 'work' AND work_run_id IS NOT NULL
         AND pair_session_id IS NULL AND pair_run_id IS NULL)
    ),
    ADD CONSTRAINT docket_execution_attempts_work_run_id_fkey
        FOREIGN KEY (work_run_id) REFERENCES bear_work_runs (id) ON DELETE CASCADE;

CREATE UNIQUE INDEX docket_execution_attempts_live_pair_owner_idx
    ON docket_execution_attempts (pair_session_id)
    WHERE state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')
      AND owner_kind = 'pair';
CREATE UNIQUE INDEX docket_execution_attempts_live_work_owner_idx
    ON docket_execution_attempts (work_run_id)
    WHERE state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')
      AND owner_kind = 'work';
