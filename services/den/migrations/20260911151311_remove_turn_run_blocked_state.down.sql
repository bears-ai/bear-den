ALTER TABLE turn_runs
    DROP CONSTRAINT IF EXISTS turn_runs_state_check;

ALTER TABLE turn_runs
    ADD CONSTRAINT turn_runs_state_check CHECK (state IN (
        'accepted',
        'running',
        'waiting_for_client',
        'continuing',
        'blocked',
        'completed',
        'failed',
        'cancelled'
    ));

UPDATE turn_runs run
SET state = 'blocked',
    updated_at = NOW()
WHERE run.state = 'failed'
  AND EXISTS (
      SELECT 1
      FROM bearwire_events event
      WHERE event.event_type = 'run.failed'
        AND event.event_json #>> '{data,legacy_terminal_kind}' = 'blocked'
        AND event.event_json ->> 'run_id' = run.run_id
  );

UPDATE bearwire_events
SET event_type = 'run.blocked',
    event_json = jsonb_set(
        event_json #- '{data,legacy_terminal_kind}',
        '{type}',
        to_jsonb('run.blocked'::text),
        true
    )
WHERE event_type = 'run.failed'
  AND event_json #>> '{data,legacy_terminal_kind}' = 'blocked';
