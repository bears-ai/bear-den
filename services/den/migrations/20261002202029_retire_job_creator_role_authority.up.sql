ALTER TABLE bear_jobs
    DROP CONSTRAINT bear_jobs_created_by_role_check,
    ADD CONSTRAINT bear_jobs_creator_provenance_check
        CHECK (btrim(created_by_role) <> '');
