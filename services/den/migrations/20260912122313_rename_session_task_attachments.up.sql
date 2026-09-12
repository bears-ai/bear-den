ALTER TABLE bear_pair_task_attachments
    RENAME TO bear_session_task_attachments;

ALTER INDEX idx_bear_pair_task_attachments_active_session
    RENAME TO idx_bear_session_task_attachments_active_session;

COMMENT ON TABLE bear_session_task_attachments IS
    'Temporary client-session ownership of Docket tasks, independent of Bear stance.';
