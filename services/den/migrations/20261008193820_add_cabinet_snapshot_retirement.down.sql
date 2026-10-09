-- Refuse to discard retirement receipts or reintroduce retention around retired payloads.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM artifact_links WHERE retention_released_at IS NOT NULL) THEN
        RAISE EXCEPTION 'snapshot retirement receipts exist; downgrade requires an explicit audit-preserving migration';
    END IF;
END $$;
DROP TRIGGER artifacts_snapshot_retirement_commit ON artifacts;
DROP TRIGGER artifact_links_snapshot_retirement_commit ON artifact_links;
DROP FUNCTION verify_snapshot_retirement_commit();
DROP TRIGGER artifacts_snapshot_immutable ON artifacts;
DROP FUNCTION protect_snapshot_registry();
DROP TRIGGER snapshot_reference_closed ON artifacts;
DROP TRIGGER snapshot_reference_closed ON bear_run_checkpoints;
DROP TRIGGER snapshot_reference_closed ON docket_turn_attempts;
DROP TRIGGER snapshot_reference_closed ON docket_turn_claims;
DROP TRIGGER snapshot_reference_closed ON artifact_json_payloads;
DROP TRIGGER snapshot_reference_closed ON cabinet_source_links;
DROP TRIGGER snapshot_reference_closed ON bear_task_events;
DROP TRIGGER snapshot_reference_closed ON bear_job_events;
DROP TRIGGER snapshot_reference_closed ON bear_job_runs;
DROP TRIGGER snapshot_reference_closed ON bear_work_runs;
DROP TRIGGER snapshot_reference_closed ON bear_job_criteria_state;
DROP TRIGGER snapshot_reference_closed ON bear_task_run_state;
DROP TRIGGER snapshot_reference_closed ON bear_docket_entries;
DROP TRIGGER snapshot_reference_closed ON docket_task_completion_receipts;
DROP TRIGGER snapshot_reference_closed ON docket_checkpoint_directives;
DROP TRIGGER artifact_links_reference_closed ON artifact_links;
DROP FUNCTION refuse_closed_artifact_link();
DROP FUNCTION refuse_retired_artifact_reference();
DROP TRIGGER artifact_json_snapshot_immutable ON artifact_json_payloads;
DROP FUNCTION protect_snapshot_payload();
DROP TRIGGER artifact_links_snapshot_citation ON artifact_links;
DROP FUNCTION protect_snapshot_citation();
CREATE OR REPLACE FUNCTION protect_cabinet_retained_artifact() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF (TG_OP='DELETE' OR NEW.lifecycle IN('deleted','expired')) AND EXISTS(SELECT 1 FROM artifact_links WHERE artifact_id=OLD.id AND target_kind IN('cabinet_item','cabinet_snapshot')) THEN
  RAISE EXCEPTION 'artifact is retained by Cabinet; detach retained links before removal' USING ERRCODE='23514';
 END IF;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END;
$$;
DROP FUNCTION cabinet_snapshot_job_settled(UUID);
DROP FUNCTION docket_job_can_release_private_source(UUID);
DROP FUNCTION cabinet_snapshot_is_simple(UUID);
DROP FUNCTION docket_job_has_foreign_requirements(UUID);
DROP FUNCTION artifact_has_required_evidence(UUID);
DROP FUNCTION artifact_has_cabinet_retention(UUID);
ALTER TABLE artifact_links DROP CONSTRAINT artifact_links_snapshot_release,
    DROP COLUMN retirement_fingerprint, DROP COLUMN retention_release_reason,
    DROP COLUMN retention_released_by_user_id, DROP COLUMN retention_released_at;
