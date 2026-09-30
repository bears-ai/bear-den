-- A queued reflection run may be scheduled for the future. Existing runs are
-- immediately due; only bounded Curate synthesis retries use a later value.
ALTER TABLE bear_reflection_runs
    ADD COLUMN available_at TIMESTAMPTZ NOT NULL DEFAULT now();
CREATE INDEX idx_memory_curate_due_runs ON bear_reflection_runs (available_at, bear_id)
    WHERE lane = 'memory_curate' AND status = 'queued';
