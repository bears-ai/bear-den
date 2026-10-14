-- The first discussion migration is already applied. Extend its guard without
-- rewriting its checksum, including protection during a multi-step downgrade.
LOCK TABLE cabinet_discussion_entries IN ACCESS EXCLUSIVE MODE;

CREATE OR REPLACE FUNCTION protect_cabinet_discussion_entry()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NOT EXISTS (
            SELECT 1 FROM _sqlx_migrations
            WHERE version = 20261013194208 AND success
        ) THEN
            RAISE EXCEPTION 'Cabinet discussion writes are paused during schema downgrade; reapply the retention migration'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'Cabinet discussion entries are immutable' USING ERRCODE = '23514';
END;
$$;

DROP TRIGGER cabinet_discussion_entries_immutable ON cabinet_discussion_entries;
CREATE TRIGGER cabinet_discussion_entries_immutable
    BEFORE INSERT OR UPDATE OR DELETE ON cabinet_discussion_entries
    FOR EACH ROW EXECUTE FUNCTION protect_cabinet_discussion_entry();

CREATE TRIGGER cabinet_discussion_entries_no_truncate
    BEFORE TRUNCATE ON cabinet_discussion_entries
    FOR EACH STATEMENT EXECUTE FUNCTION protect_cabinet_discussion_entry();
