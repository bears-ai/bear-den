DROP INDEX artifacts_cabinet_cleanup_queue;
ALTER TABLE artifacts DROP CONSTRAINT artifact_content_removed_terminal;
ALTER TABLE artifacts DROP COLUMN content_removed_at;
