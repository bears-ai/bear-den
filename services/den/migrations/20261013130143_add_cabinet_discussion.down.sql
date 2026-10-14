-- Rollback may remove the empty feature, but must not silently destroy discussion audit.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM cabinet_discussion_entries) THEN
        RAISE EXCEPTION 'Cabinet discussion entries exist; preserve their audit before rollback';
    END IF;
END;
$$;

DROP TRIGGER cabinet_discussion_entries_immutable ON cabinet_discussion_entries;
DROP FUNCTION protect_cabinet_discussion_entry();
DROP TABLE cabinet_discussion_entries;
