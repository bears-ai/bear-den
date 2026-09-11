ALTER TABLE turn_runs
    DROP CONSTRAINT IF EXISTS turn_runs_state_check;

UPDATE bearwire_events
SET event_type = 'run.failed',
    event_json = jsonb_set(
        jsonb_set(event_json, '{type}', to_jsonb('run.failed'::text), true),
        '{data,legacy_terminal_kind}',
        to_jsonb('blocked'::text),
        true
    )
WHERE event_type = 'run.blocked';

UPDATE turn_runs
SET state = 'failed',
    updated_at = NOW()
WHERE state = 'blocked';

ALTER TABLE turn_runs
    ADD CONSTRAINT turn_runs_state_check CHECK (state IN (
        'accepted',
        'running',
        'waiting_for_client',
        'continuing',
        'completed',
        'failed',
        'cancelled'
    ));
