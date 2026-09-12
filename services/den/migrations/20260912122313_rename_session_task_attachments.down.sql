ALTER INDEX idx_bear_session_task_attachments_active_session
    RENAME TO idx_bear_pair_task_attachments_active_session;

ALTER TABLE bear_session_task_attachments
    RENAME TO bear_pair_task_attachments;
