-- A client session ID is used as a run/event binding outside client_sessions. Older
-- installations may already have duplicates, so a global UNIQUE constraint would
-- discard history or fail this migration. Quarantine those existing collisions and
-- forbid any new cross-user or cross-Bear binding, including concurrent inserts.
CREATE FUNCTION guard_client_session_id_ownership() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- Serialize contenders for the same opaque ID before testing the canonical
    -- binding. Hash collisions only serialize unrelated IDs; they cannot grant access.
    PERFORM pg_advisory_xact_lock(hashtextextended(NEW.client_session_id, 0));
    IF EXISTS (
        SELECT 1 FROM client_sessions existing
        WHERE existing.client_session_id = NEW.client_session_id
          AND (existing.user_id, existing.bear_id)
              IS DISTINCT FROM (NEW.user_id, NEW.bear_id)
    ) THEN
        RAISE EXCEPTION 'client session ID is already bound to a different owner'
            USING ERRCODE = '23505',
                  CONSTRAINT = 'client_sessions_global_owner_guard';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER client_sessions_global_owner_guard
BEFORE INSERT OR UPDATE OF client_session_id, user_id, bear_id ON client_sessions
FOR EACH ROW EXECUTE FUNCTION guard_client_session_id_ownership();
