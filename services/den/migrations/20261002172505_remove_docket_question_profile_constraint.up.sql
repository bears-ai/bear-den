-- Docket questions are authorized by a live owned human session, not by an audit role.
ALTER TABLE bear_docket_entries
    DROP CONSTRAINT bear_docket_entries_check3;
