CREATE TABLE job_cabinet_refs(job_id UUID PRIMARY KEY REFERENCES bear_jobs(id) ON DELETE CASCADE,cabinet_ref TEXT REFERENCES cabinet_items(cabinet_ref) ON DELETE RESTRICT,revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0));
CREATE FUNCTION protect_cabinet_retained_artifact() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF (TG_OP='DELETE' OR NEW.lifecycle IN('deleted','expired')) AND EXISTS(SELECT 1 FROM artifact_links WHERE artifact_id=OLD.id AND target_kind IN('cabinet_item','cabinet_snapshot')) THEN
  RAISE EXCEPTION 'artifact is retained by Cabinet; detach retained links before removal' USING ERRCODE='23514';
 END IF;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER artifacts_cabinet_retention BEFORE DELETE OR UPDATE OF lifecycle ON artifacts FOR EACH ROW EXECUTE FUNCTION protect_cabinet_retained_artifact();
