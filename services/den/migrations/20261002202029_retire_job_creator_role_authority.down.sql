-- Rollback refuses incompatible audit rows rather than deleting or relabeling them.
ALTER TABLE bear_jobs
    DROP CONSTRAINT bear_jobs_creator_provenance_check,
    ADD CONSTRAINT bear_jobs_created_by_role_check
        CHECK (created_by_role IN ('chat', 'pair', 'ui'));
