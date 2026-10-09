-- Restore the preceding guards and trigger event sets without deleting audit data.
DROP TRIGGER artifacts_snapshot_retirement_commit ON artifacts;
DROP TRIGGER artifact_links_snapshot_retirement_commit ON artifact_links;
CREATE CONSTRAINT TRIGGER artifacts_snapshot_retirement_commit AFTER UPDATE ON artifacts
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_snapshot_retirement_commit();
CREATE CONSTRAINT TRIGGER artifact_links_snapshot_retirement_commit AFTER UPDATE ON artifact_links
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_snapshot_retirement_commit();

CREATE OR REPLACE FUNCTION protect_snapshot_citation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF OLD.target_kind='cabinet_snapshot' AND EXISTS (SELECT 1 FROM artifacts WHERE id=OLD.artifact_id) THEN
            RAISE EXCEPTION 'snapshot citation is immutable' USING ERRCODE='23514', CONSTRAINT='snapshot_citation_immutable';
        END IF;
        RETURN OLD;
    END IF;
    IF TG_OP='UPDATE' AND (OLD.target_kind='cabinet_snapshot' OR NEW.target_kind='cabinet_snapshot') THEN
        IF OLD.retention_released_at IS NOT NULL AND NEW.created_by_user_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM users WHERE id=OLD.created_by_user_id)
           AND (to_jsonb(OLD)-'created_by_user_id') = (to_jsonb(NEW)-'created_by_user_id') THEN
            RETURN NEW;
        END IF;
        IF (to_jsonb(OLD)-ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'])
           IS DISTINCT FROM
           (to_jsonb(NEW)-ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'])
           OR (OLD.retention_released_at IS NOT NULL AND to_jsonb(OLD) IS DISTINCT FROM to_jsonb(NEW)) THEN
            RAISE EXCEPTION 'snapshot citation is immutable' USING ERRCODE='23514', CONSTRAINT='snapshot_citation_immutable';
        END IF;
        IF OLD.retention_released_at IS NULL AND NEW.retention_released_at IS NOT NULL THEN
            PERFORM 1 FROM artifacts WHERE id=OLD.artifact_id FOR UPDATE;
            IF NOT cabinet_snapshot_is_simple(OLD.artifact_id) OR NOT cabinet_snapshot_job_settled(OLD.artifact_id)
               OR NOT EXISTS (SELECT 1 FROM artifacts WHERE id=OLD.artifact_id
                    AND lifecycle='finalized' AND created_by_user_id=NEW.retention_released_by_user_id) THEN
                RAISE EXCEPTION 'snapshot is not eligible for retirement' USING ERRCODE='23514', CONSTRAINT='snapshot_retirement_eligible';
            END IF;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
DROP TRIGGER artifact_links_snapshot_citation ON artifact_links;
CREATE TRIGGER artifact_links_snapshot_citation BEFORE UPDATE OR DELETE ON artifact_links
    FOR EACH ROW EXECUTE FUNCTION protect_snapshot_citation();

CREATE OR REPLACE FUNCTION protect_snapshot_registry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.kind='cabinet_document_snapshot' AND OLD.lifecycle IN ('finalized','deleted') THEN
        IF (to_jsonb(OLD)-ARRAY['lifecycle','deleted_at','updated_at','created_by_user_id']) IS DISTINCT FROM
           (to_jsonb(NEW)-ARRAY['lifecycle','deleted_at','updated_at','created_by_user_id'])
           OR (OLD.created_by_user_id IS DISTINCT FROM NEW.created_by_user_id AND NOT (
               OLD.lifecycle='deleted' AND NEW.created_by_user_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM users WHERE id=OLD.created_by_user_id)))
           OR (OLD.lifecycle='deleted' AND NEW.lifecycle<>'deleted')
           OR (NEW.lifecycle='deleted' AND NOT EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=OLD.id
               AND l.target_kind='cabinet_snapshot' AND l.retention_released_at IS NOT NULL)) THEN
            RAISE EXCEPTION 'snapshot registry identity is immutable' USING ERRCODE='23514', CONSTRAINT='snapshot_registry_immutable';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION docket_job_has_foreign_requirements(job UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM bear_jobs j WHERE j.id=job AND (
        EXISTS (SELECT 1 FROM bear_tasks t WHERE t.job_id=j.id AND t.bear_id<>j.bear_id)
        OR EXISTS (SELECT 1 FROM bear_work_runs w JOIN bear_job_runs r ON r.id=w.job_run_id
            WHERE w.job_id=j.id AND (w.bear_id<>j.bear_id OR r.job_id<>j.id))
        OR EXISTS (SELECT 1 FROM docket_execution_attempts e JOIN bear_tasks t ON t.id=e.task_id
            WHERE t.job_id=j.id AND e.bear_id<>j.bear_id)
        OR EXISTS (SELECT 1 FROM bear_job_runs r WHERE r.id=j.current_run_id AND r.job_id<>j.id)
    ))
$$;
DROP FUNCTION docket_run_has_foreign_requirements(UUID);
