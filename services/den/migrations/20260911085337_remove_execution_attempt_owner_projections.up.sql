-- Generic binding/host columns are the sole writable execution authority.
-- work_run_id remains only as a generated referential-integrity projection so
-- PostgreSQL can retain its foreign key and ON DELETE CASCADE behavior.
DROP INDEX IF EXISTS docket_execution_attempts_live_pair_owner_idx;
DROP INDEX IF EXISTS docket_execution_attempts_live_work_owner_idx;

ALTER TABLE docket_execution_attempts
    DROP CONSTRAINT IF EXISTS docket_execution_attempts_check,
    DROP CONSTRAINT IF EXISTS docket_execution_attempts_owner_kind_check,
    DROP CONSTRAINT IF EXISTS docket_execution_attempts_work_run_id_fkey,
    DROP COLUMN owner_kind,
    DROP COLUMN pair_session_id,
    DROP COLUMN pair_run_id,
    DROP COLUMN work_run_id;

ALTER TABLE docket_execution_attempts
    ADD COLUMN work_run_id UUID GENERATED ALWAYS AS (
        CASE WHEN binding_kind = 'work_assignment' THEN binding_id::uuid END
    ) STORED,
    ADD CONSTRAINT docket_execution_attempts_work_run_id_fkey
        FOREIGN KEY (work_run_id) REFERENCES bear_work_runs (id) ON DELETE CASCADE;

COMMENT ON COLUMN docket_execution_attempts.work_run_id IS
    'Generated referential-integrity projection of work-assignment binding_id; never execution authority.';
