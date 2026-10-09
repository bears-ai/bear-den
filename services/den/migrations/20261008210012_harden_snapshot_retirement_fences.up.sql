-- FK target rows are locked by the lifecycle service before these reverse checks.
-- Other Jobs/Work runs must not be SET NULL or cascaded as a side effect of removal.
CREATE FUNCTION docket_run_has_foreign_requirements(run UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM bear_job_runs r JOIN bear_jobs owner ON owner.id=r.job_id
        WHERE r.id=run AND (
            EXISTS (SELECT 1 FROM bear_jobs other
                WHERE other.current_run_id=r.id AND other.id<>owner.id)
            OR EXISTS (SELECT 1 FROM bear_work_runs other
                WHERE other.job_run_id=r.id
                  AND (other.job_id<>owner.id OR other.bear_id<>owner.bear_id))
        )
    )
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
        OR EXISTS (SELECT 1 FROM bear_job_runs r WHERE r.job_id=j.id
            AND docket_run_has_foreign_requirements(r.id))
    ))
$$;

CREATE OR REPLACE FUNCTION protect_snapshot_registry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.kind='cabinet_document_snapshot' AND OLD.lifecycle='finalized' THEN
        -- A finalized row may be a no-op, or retire with its validated original citation.
        IF NEW.lifecycle='finalized' AND to_jsonb(NEW) IS NOT DISTINCT FROM to_jsonb(OLD) THEN
            RETURN NEW;
        END IF;
        IF NEW.lifecycle<>'deleted'
           OR (to_jsonb(OLD)-ARRAY['lifecycle','deleted_at','updated_at']) IS DISTINCT FROM
              (to_jsonb(NEW)-ARRAY['lifecycle','deleted_at','updated_at'])
           OR NEW.deleted_at IS NULL OR OLD.created_by_user_id IS NULL
           OR NOT cabinet_snapshot_is_simple(OLD.id)
           OR NOT cabinet_snapshot_job_settled(OLD.id)
           OR NOT EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id=OLD.id
                AND l.target_kind='cabinet_snapshot' AND l.role='citation'
                AND l.created_by_user_id=OLD.created_by_user_id
                AND l.retention_released_by_user_id=OLD.created_by_user_id
                AND l.retention_released_at IS NOT NULL) THEN
            RAISE EXCEPTION 'snapshot registry identity is immutable'
                USING ERRCODE='23514', CONSTRAINT='snapshot_registry_immutable';
        END IF;
    ELSIF OLD.kind='cabinet_document_snapshot' AND OLD.lifecycle='deleted' THEN
        -- The receipt preserves the historical actor if a later identity deletion nulls its FK.
        IF (to_jsonb(OLD)-ARRAY['updated_at','created_by_user_id']) IS DISTINCT FROM
           (to_jsonb(NEW)-ARRAY['updated_at','created_by_user_id'])
           OR (OLD.created_by_user_id IS DISTINCT FROM NEW.created_by_user_id AND NOT (
               NEW.created_by_user_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM users WHERE id=OLD.created_by_user_id))) THEN
            RAISE EXCEPTION 'snapshot registry identity is immutable'
                USING ERRCODE='23514', CONSTRAINT='snapshot_registry_immutable';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION protect_snapshot_citation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='INSERT' THEN
        IF NEW.retention_released_at IS NOT NULL OR NEW.retention_released_by_user_id IS NOT NULL
           OR NEW.retention_release_reason IS NOT NULL OR NEW.retirement_fingerprint IS NOT NULL THEN
            RAISE EXCEPTION 'retirement receipts must release an existing citation'
                USING ERRCODE='23514', CONSTRAINT='snapshot_receipt_insert';
        END IF;
        RETURN NEW;
    END IF;
    IF TG_OP='DELETE' THEN
        IF OLD.target_kind='cabinet_snapshot' AND EXISTS (SELECT 1 FROM artifacts WHERE id=OLD.artifact_id) THEN
            RAISE EXCEPTION 'snapshot citation is immutable' USING ERRCODE='23514', CONSTRAINT='snapshot_citation_immutable';
        END IF;
        RETURN OLD;
    END IF;
    IF OLD.target_kind='cabinet_snapshot' OR NEW.target_kind='cabinet_snapshot' THEN
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
CREATE TRIGGER artifact_links_snapshot_citation BEFORE INSERT OR UPDATE OR DELETE ON artifact_links
    FOR EACH ROW EXECUTE FUNCTION protect_snapshot_citation();

DROP TRIGGER artifacts_snapshot_retirement_commit ON artifacts;
DROP TRIGGER artifact_links_snapshot_retirement_commit ON artifact_links;
CREATE CONSTRAINT TRIGGER artifacts_snapshot_retirement_commit AFTER INSERT OR UPDATE ON artifacts
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_snapshot_retirement_commit();
CREATE CONSTRAINT TRIGGER artifact_links_snapshot_retirement_commit AFTER INSERT OR UPDATE ON artifact_links
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_snapshot_retirement_commit();
