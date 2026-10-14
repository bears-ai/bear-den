-- Never remove audit protection from a populated table. Taking the lock first
-- serializes this check with in-flight posts and prevents a check/drop race.
LOCK TABLE cabinet_discussion_entries IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM cabinet_discussion_entries) THEN
        RAISE EXCEPTION 'Cabinet discussion entries exist; preserve their audit before rollback';
    END IF;
END;
$$;

DROP TRIGGER cabinet_discussion_entries_no_truncate ON cabinet_discussion_entries;

-- Deliberately retain the INSERT gate on the original immutable trigger. SQLx
-- removes this migration's bookkeeping row at commit, pausing later posts even
-- between rollback transactions. The original discussion down script can then
-- safely check emptiness and drop the original trigger/function/table; reapplying
-- this migration restores normal posts if the operator stops at this downgrade.
