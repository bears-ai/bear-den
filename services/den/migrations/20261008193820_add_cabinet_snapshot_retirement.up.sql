-- Retirement is not erasure. The citation and its immutable original metadata survive.
ALTER TABLE artifact_links
    ADD COLUMN retention_released_at TIMESTAMPTZ,
    -- Historical actor identity, deliberately not SET NULL when that identity is deleted.
    ADD COLUMN retention_released_by_user_id INTEGER,
    ADD COLUMN retention_release_reason TEXT,
    ADD COLUMN retirement_fingerprint TEXT,
    ADD CONSTRAINT artifact_links_snapshot_release CHECK (
        (retention_released_at IS NULL AND retention_released_by_user_id IS NULL
         AND retention_release_reason IS NULL AND retirement_fingerprint IS NULL)
        OR (target_kind = 'cabinet_snapshot' AND role = 'citation'
            AND retention_released_at IS NOT NULL AND retention_released_by_user_id IS NOT NULL
            AND retention_release_reason IS NOT NULL
            AND length(btrim(retention_release_reason)) BETWEEN 1 AND 2000
            AND retirement_fingerprint IS NOT NULL
            AND retirement_fingerprint ~ '^[0-9a-f]{64}$')
    );

CREATE FUNCTION artifact_has_cabinet_retention(artifact UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM artifact_links
        WHERE artifact_id = artifact AND target_kind IN ('cabinet_item','cabinet_snapshot')
          AND retention_released_at IS NULL)
$$;

-- Exact reference values in canonical evidence fields; never substring-match rendered prose.
CREATE FUNCTION artifact_has_required_evidence(artifact UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM artifacts a WHERE a.id = artifact AND (
            EXISTS (SELECT 1 FROM docket_checkpoint_directives d WHERE d.acknowledged_artifact_ref = a.artifact_ref)
            OR EXISTS (SELECT 1 FROM docket_task_completion_receipts r WHERE r.primary_output_ref = a.artifact_ref
                OR jsonb_path_exists(r.validation, '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_docket_entries e WHERE jsonb_path_exists(e.evidence_refs,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_task_run_state s WHERE jsonb_path_exists(s.result_refs,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_job_criteria_state s WHERE jsonb_path_exists(s.evidence,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_work_runs r WHERE jsonb_path_exists(r.result_refs,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_job_runs r WHERE jsonb_path_exists(r.outcome,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_job_events e WHERE jsonb_path_exists(e.payload,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM bear_task_events e WHERE jsonb_path_exists(e.payload,
                '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
            OR EXISTS (SELECT 1 FROM docket_turn_claims c WHERE jsonb_path_exists(c.expected_versions,
                            '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
                        OR EXISTS (SELECT 1 FROM docket_turn_attempts t WHERE jsonb_path_exists(to_jsonb(t),
                            '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
                        OR EXISTS (SELECT 1 FROM bear_run_checkpoints c WHERE jsonb_path_exists(c.request,
                            '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref))
                            OR jsonb_path_exists(c.response, '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
                        OR EXISTS (SELECT 1 FROM artifacts other WHERE other.id<>a.id AND (
                            jsonb_path_exists(other.provenance, '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref))
                            OR jsonb_path_exists(other.metadata, '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref))))
                        OR EXISTS (SELECT 1 FROM cabinet_source_links s WHERE s.source_kind = 'artifact' AND s.locator = a.artifact_ref)
            OR EXISTS (SELECT 1 FROM artifact_json_payloads p WHERE p.artifact_id <> a.id
                AND jsonb_path_exists(p.payload, '$.** ? (@ == $ref)', jsonb_build_object('ref',a.artifact_ref)))
        )
    )
$$;

-- Existing schemas have single-column FKs; historical cross-Bear control references
-- are requirements, not evidence that an owning workflow may silently cascade away.
CREATE FUNCTION docket_job_has_foreign_requirements(job UUID) RETURNS BOOLEAN
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

-- Only the original citation and at most one same-Bear Job source link are simple.
CREATE FUNCTION cabinet_snapshot_is_simple(artifact UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM artifacts a JOIN artifact_json_payloads p ON p.artifact_id = a.id
        WHERE a.id = artifact AND a.kind = 'cabinet_document_snapshot' AND a.storage_kind = 'db_text'
          AND a.visibility = 'same_user' AND a.expires_at IS NULL
                    AND a.provenance = jsonb_build_object('cabinet_ref',p.payload->>'cabinet_ref','version_ref',p.payload->>'version_ref')
          AND (a.created_by_user_id IS NOT NULL OR EXISTS (SELECT 1 FROM artifact_links receipt
                        WHERE receipt.artifact_id=a.id AND receipt.retention_released_at IS NOT NULL))
          AND (SELECT count(*) FROM artifact_links l WHERE l.artifact_id = a.id AND l.target_kind = 'cabinet_snapshot'
               AND l.role = 'citation' AND l.target_id = p.payload->>'version_ref'
               AND l.created_by_user_id IS NOT DISTINCT FROM a.created_by_user_id
               AND l.metadata = jsonb_build_object('cabinet_ref',p.payload->>'cabinet_ref')) = 1
          AND (SELECT count(*) FROM artifact_links l WHERE l.artifact_id = a.id AND l.target_kind = 'docket_job') <= 1
          AND NOT EXISTS (SELECT 1 FROM artifact_links l WHERE l.artifact_id = a.id AND NOT COALESCE((
              (l.target_kind = 'cabinet_snapshot' AND l.role = 'citation'
               AND l.target_id = p.payload->>'version_ref' AND l.created_by_user_id IS NOT DISTINCT FROM a.created_by_user_id
               AND l.metadata = jsonb_build_object('cabinet_ref',p.payload->>'cabinet_ref'))
              OR (l.target_kind = 'docket_job' AND l.role = 'source' AND l.metadata = '{}'::jsonb
                  AND l.created_by_user_id IS NOT DISTINCT FROM a.created_by_user_id
                  AND EXISTS (SELECT 1 FROM bear_jobs j WHERE j.id::text = l.target_id AND j.bear_id = a.bear_id
                                        AND NOT docket_job_has_foreign_requirements(j.id)))
          ),false)) AND NOT artifact_has_required_evidence(a.id))
$$;

-- Docket owns the completion projection: lifecycle_intent, current-run task and criterion
-- evidence, not a separately writable Job status. Execution liveness is checked below too.
CREATE FUNCTION docket_job_can_release_private_source(job UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM bear_jobs j WHERE j.id=job AND (
        j.lifecycle_intent='cancelled'
        OR (j.lifecycle_intent IS NULL
            AND EXISTS (SELECT 1 FROM bear_job_runs r WHERE r.id=j.current_run_id AND r.job_id=j.id
                AND r.state IN ('completed','failed','cancelled'))
            AND (EXISTS (SELECT 1 FROM bear_tasks t WHERE t.job_id=j.id)
                 OR EXISTS (SELECT 1 FROM bear_job_criteria c WHERE c.job_id=j.id))
            AND NOT EXISTS (SELECT 1 FROM bear_tasks t LEFT JOIN bear_task_run_state s
                ON s.task_id=t.id AND s.run_id=j.current_run_id WHERE t.job_id=j.id
                AND (s.status IS NULL OR s.status NOT IN ('done','cancelled')))
            AND NOT EXISTS (SELECT 1 FROM bear_job_criteria c LEFT JOIN bear_job_criteria_state s
                ON s.criterion_id=c.id AND s.run_id=j.current_run_id WHERE c.job_id=j.id
                AND (s.status IS NULL OR s.status NOT IN ('met','waived')))
        )))
$$;

CREATE FUNCTION cabinet_snapshot_job_settled(artifact UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT NOT EXISTS (SELECT 1 FROM artifact_links l JOIN artifacts a ON a.id = l.artifact_id
        LEFT JOIN bear_jobs j ON j.id::text = l.target_id AND j.bear_id = a.bear_id
        WHERE a.id = artifact AND l.target_kind = 'docket_job' AND (
            j.id IS NULL OR NOT docket_job_can_release_private_source(j.id)
            OR EXISTS (SELECT 1 FROM bear_job_runs r WHERE r.job_id = j.id AND r.state NOT IN ('completed','failed','cancelled'))
                        OR EXISTS (SELECT 1 FROM bear_job_runs r WHERE r.id=j.current_run_id
                            AND (r.job_id<>j.id OR r.state NOT IN ('completed','failed','cancelled')))
            OR EXISTS (SELECT 1 FROM bear_work_runs r WHERE r.job_id = j.id
                AND (r.state NOT IN ('stalled','succeeded','blocked','failed','cancelled','timed_out') OR r.finished_at IS NULL))
            OR EXISTS (SELECT 1 FROM docket_execution_attempts e JOIN bear_tasks t ON t.id = e.task_id
                WHERE t.job_id = j.id AND e.state NOT IN ('settled','released'))
            OR EXISTS (SELECT 1 FROM docket_turn_claims c WHERE c.job_id = j.id AND c.state NOT IN ('settled','abandoned'))
                        OR EXISTS (SELECT 1 FROM docket_turn_attempts t JOIN docket_routing_decisions d ON d.id=t.routing_decision_id
                            WHERE d.job_id=j.id AND t.state NOT IN ('settled','abandoned'))
        ))
$$;

CREATE OR REPLACE FUNCTION protect_cabinet_retained_artifact() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF (TG_OP = 'DELETE' OR NEW.lifecycle IN ('deleted','expired'))
       AND artifact_has_cabinet_retention(OLD.id) THEN
        RAISE EXCEPTION 'artifact is retained by Cabinet' USING ERRCODE = '23514', CONSTRAINT = 'artifacts_cabinet_retention';
    END IF;
    IF TG_OP = 'DELETE' THEN
        IF OLD.kind='cabinet_document_snapshot' AND EXISTS (SELECT 1 FROM bears WHERE id=OLD.bear_id) THEN
            RAISE EXCEPTION 'snapshot audit belongs to a surviving Bear' USING ERRCODE='23514', CONSTRAINT='snapshot_audit_owned';
        END IF;
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

-- Citation originals and receipts cannot be rewritten, even through a generic attach upsert.
CREATE FUNCTION protect_snapshot_citation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF OLD.target_kind='cabinet_snapshot' AND EXISTS (SELECT 1 FROM artifacts WHERE id=OLD.artifact_id) THEN
            RAISE EXCEPTION 'snapshot citation is immutable' USING ERRCODE='23514', CONSTRAINT='snapshot_citation_immutable';
        END IF;
        RETURN OLD;
    END IF;
    IF TG_OP = 'UPDATE' AND (OLD.target_kind = 'cabinet_snapshot' OR NEW.target_kind = 'cabinet_snapshot') THEN
        -- Identity deletion can null its FK after retirement; the receipt retains the actor.
        IF OLD.retention_released_at IS NOT NULL AND NEW.created_by_user_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM users WHERE id=OLD.created_by_user_id)
           AND (to_jsonb(OLD)-'created_by_user_id') = (to_jsonb(NEW)-'created_by_user_id') THEN
            RETURN NEW;
        END IF;
        IF (to_jsonb(OLD) - ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'])
           IS DISTINCT FROM
           (to_jsonb(NEW) - ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'])
           OR (OLD.retention_released_at IS NOT NULL AND to_jsonb(OLD) IS DISTINCT FROM to_jsonb(NEW)) THEN
            RAISE EXCEPTION 'snapshot citation is immutable' USING ERRCODE = '23514', CONSTRAINT = 'snapshot_citation_immutable';
        END IF;
        IF OLD.retention_released_at IS NULL AND NEW.retention_released_at IS NOT NULL THEN
            PERFORM 1 FROM artifacts WHERE id = OLD.artifact_id FOR UPDATE;
            IF NOT cabinet_snapshot_is_simple(OLD.artifact_id) OR NOT cabinet_snapshot_job_settled(OLD.artifact_id)
               OR NOT EXISTS (SELECT 1 FROM artifacts WHERE id = OLD.artifact_id
                    AND lifecycle = 'finalized' AND created_by_user_id = NEW.retention_released_by_user_id) THEN
                RAISE EXCEPTION 'snapshot is not eligible for retirement' USING ERRCODE = '23514', CONSTRAINT = 'snapshot_retirement_eligible';
            END IF;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER artifact_links_snapshot_citation BEFORE UPDATE OR DELETE ON artifact_links
    FOR EACH ROW EXECUTE FUNCTION protect_snapshot_citation();

CREATE FUNCTION protect_snapshot_payload() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM artifacts WHERE id = OLD.artifact_id AND kind = 'cabinet_document_snapshot'
               AND lifecycle IN ('finalized','deleted')) THEN
        RAISE EXCEPTION 'snapshot payload is immutable' USING ERRCODE = '23514', CONSTRAINT = 'snapshot_payload_immutable';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER artifact_json_snapshot_immutable BEFORE UPDATE OR DELETE ON artifact_json_payloads
    FOR EACH ROW EXECUTE FUNCTION protect_snapshot_payload();

-- Source writers take artifact locks after their source locks, just like retirement.
CREATE FUNCTION refuse_retired_artifact_reference() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE a RECORD; evidence JSONB;
BEGIN
    evidence := to_jsonb(NEW);
    -- Match the canonical reference fields above, not narrative titles or rendered prose.
    CASE TG_TABLE_NAME
        WHEN 'artifacts' THEN evidence := jsonb_build_object('provenance',evidence->'provenance','metadata',evidence->'metadata');
        WHEN 'artifact_json_payloads' THEN evidence := evidence->'payload';
        WHEN 'docket_checkpoint_directives' THEN evidence := evidence->'acknowledged_artifact_ref';
        WHEN 'docket_task_completion_receipts' THEN evidence := jsonb_build_object('primary_output_ref',evidence->'primary_output_ref','validation',evidence->'validation');
        WHEN 'bear_docket_entries' THEN evidence := evidence->'evidence_refs';
        WHEN 'bear_task_run_state' THEN evidence := evidence->'result_refs';
        WHEN 'bear_job_criteria_state' THEN evidence := evidence->'evidence';
        WHEN 'bear_work_runs' THEN evidence := evidence->'result_refs';
        WHEN 'bear_job_runs' THEN evidence := evidence->'outcome';
        WHEN 'bear_job_events' THEN evidence := evidence->'payload';
        WHEN 'bear_task_events' THEN evidence := evidence->'payload';
        WHEN 'bear_run_checkpoints' THEN evidence := jsonb_build_object('request',evidence->'request','response',evidence->'response');
        WHEN 'cabinet_source_links' THEN
            IF NEW.source_kind='artifact' THEN evidence := evidence->'locator'; ELSE evidence := NULL; END IF;
        WHEN 'docket_turn_claims' THEN evidence := evidence->'expected_versions';
        WHEN 'docket_turn_attempts' THEN NULL;
        ELSE RAISE EXCEPTION 'unregistered artifact reference boundary' USING ERRCODE='23514';
    END CASE;
    FOR a IN SELECT id FROM artifacts WHERE jsonb_path_exists(evidence, '$.** ? (@ == $ref)', jsonb_build_object('ref',artifact_ref))
        ORDER BY id
    LOOP
        PERFORM 1 FROM artifacts WHERE id = a.id FOR SHARE;
        IF NOT EXISTS (SELECT 1 FROM artifacts WHERE id = a.id AND lifecycle NOT IN ('deleted','expired')) THEN
            RAISE EXCEPTION 'retired artifact cannot acquire references' USING ERRCODE = '23514', CONSTRAINT = 'snapshot_reference_closed';
        END IF;
    END LOOP;
    RETURN NEW;
END;
$$;
CREATE FUNCTION refuse_closed_artifact_link() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE state TEXT;
BEGIN
    SELECT lifecycle INTO state FROM artifacts WHERE id = NEW.artifact_id FOR SHARE;
    IF state IN ('deleted','expired') THEN
        RAISE EXCEPTION 'retired artifact cannot acquire links' USING ERRCODE = '23514', CONSTRAINT = 'artifact_reference_closed';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER artifact_links_reference_closed BEFORE INSERT OR UPDATE OF artifact_id,target_kind,target_id,role,metadata ON artifact_links
    FOR EACH ROW EXECUTE FUNCTION refuse_closed_artifact_link();

CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON docket_checkpoint_directives FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON docket_task_completion_receipts FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_docket_entries FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_task_run_state FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_job_criteria_state FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_work_runs FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_job_runs FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_job_events FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_task_events FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON cabinet_source_links FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON artifact_json_payloads FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON bear_run_checkpoints FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON docket_turn_attempts FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE ON docket_turn_claims FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();
CREATE TRIGGER snapshot_reference_closed BEFORE INSERT OR UPDATE OF provenance,metadata ON artifacts FOR EACH ROW EXECUTE FUNCTION refuse_retired_artifact_reference();

CREATE FUNCTION protect_snapshot_registry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
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
CREATE TRIGGER artifacts_snapshot_immutable BEFORE UPDATE ON artifacts FOR EACH ROW EXECUTE FUNCTION protect_snapshot_registry();

-- Receipt release and logical retirement are one transaction, never an independently
-- writable half-retired state that still serves content or accepts new references.
CREATE FUNCTION verify_snapshot_retirement_commit() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE artifact UUID;
BEGIN
    IF TG_TABLE_NAME='artifacts' THEN artifact:=NEW.id; ELSE artifact:=NEW.artifact_id; END IF;
    IF EXISTS (SELECT 1 FROM artifacts a WHERE a.id=artifact AND a.kind='cabinet_document_snapshot'
        AND ((a.lifecycle='deleted') IS DISTINCT FROM EXISTS (SELECT 1 FROM artifact_links l
            WHERE l.artifact_id=a.id AND l.target_kind='cabinet_snapshot' AND l.retention_released_at IS NOT NULL))) THEN
        RAISE EXCEPTION 'snapshot receipt and lifecycle must retire atomically' USING ERRCODE='23514', CONSTRAINT='snapshot_retirement_atomic';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER artifacts_snapshot_retirement_commit AFTER UPDATE ON artifacts
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_snapshot_retirement_commit();
CREATE CONSTRAINT TRIGGER artifact_links_snapshot_retirement_commit AFTER UPDATE ON artifact_links
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_snapshot_retirement_commit();
