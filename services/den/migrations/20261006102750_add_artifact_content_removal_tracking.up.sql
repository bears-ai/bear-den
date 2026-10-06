ALTER TABLE artifacts ADD COLUMN content_removed_at TIMESTAMPTZ;
ALTER TABLE artifacts ADD CONSTRAINT artifact_content_removed_terminal
    CHECK (content_removed_at IS NULL OR lifecycle IN ('deleted', 'expired'));
CREATE INDEX artifacts_cabinet_cleanup_queue ON artifacts (updated_at, id)
    WHERE kind = 'cabinet_file' AND storage_kind = 'garage_artifacts'
      AND content_removed_at IS NULL AND expires_at IS NOT NULL;
