DROP INDEX IF EXISTS runtime_approvals_execution_request_id_idx;
ALTER TABLE runtime_approvals
    DROP COLUMN IF EXISTS consumed_at,
    DROP COLUMN IF EXISTS execution_request_id;
