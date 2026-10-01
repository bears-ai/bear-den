-- ACP one-time URL grants were previously stored as reusable, Bear-wide approvals.
-- Their provenance has no hat ID; do not carry them forward as lasting access.
UPDATE bear_web_approvals
SET revoked_at = now()
WHERE source = 'acp'
  AND scope_kind = 'url'
  AND expires_at IS NOT NULL
  AND revoked_at IS NULL;
