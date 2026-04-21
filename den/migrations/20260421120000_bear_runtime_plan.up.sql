-- Runtime workspace / memfs metadata for codepool (JSON mirrors BearRuntimePlan in codepool).
ALTER TABLE bears ADD COLUMN IF NOT EXISTS runtime_plan JSONB;

COMMENT ON COLUMN bears.runtime_plan IS 'Optional BearRuntimePlan v1 JSON for codepool provisioning (memory git remote, seeds; extensible for tool env).';
