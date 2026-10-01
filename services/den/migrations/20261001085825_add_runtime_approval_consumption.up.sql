-- A one-time ACP decision belongs to exactly one native continuation request.
-- It must never become a reusable Bear-wide web approval.
ALTER TABLE runtime_approvals
    ADD COLUMN execution_request_id UUID NULL,
    ADD COLUMN consumed_at TIMESTAMPTZ NULL;

CREATE UNIQUE INDEX runtime_approvals_execution_request_id_idx
    ON runtime_approvals (execution_request_id)
    WHERE execution_request_id IS NOT NULL;
