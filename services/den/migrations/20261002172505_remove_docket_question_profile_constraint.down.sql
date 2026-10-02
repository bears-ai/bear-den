-- This intentionally fails if non-Pair questions were recorded after the upgrade.
ALTER TABLE bear_docket_entries
    ADD CONSTRAINT bear_docket_entries_check3
    CHECK (kind <> 'question' OR by_role = 'pair');
