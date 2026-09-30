-- The previous Work decision reviewed hat memory, not the newly model-visible
-- hat identity. Do not silently widen that identity to autonomous Work. Re-enable
-- through the empty-hat or populated-hat review after inspecting both audiences.
ALTER TABLE bear_hat_work_reviews ADD COLUMN identity_sha256 TEXT
    CHECK (identity_sha256 IS NULL OR identity_sha256 ~ '^[a-f0-9]{64}$');
UPDATE bear_hats SET work_enabled = false, updated_at = NOW() WHERE work_enabled;
