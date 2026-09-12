#!/usr/bin/env sh
set -eu

RUN_ID="${1:-}"
if [ -z "$RUN_ID" ]; then
  echo "usage: $0 <bearwire-run-id>" >&2
  exit 64
fi

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
CONTAINER="${BEARS_POSTGRES_CONTAINER:-}"
DB_USER="${BEARS_POSTGRES_USER:-bears}"
DB_NAME="${BEARS_POSTGRES_DB:-den}"

if [ -z "$CONTAINER" ]; then
  CONTAINER="$(cd "$ROOT" && docker compose ps -q bears-postgres)"
fi
if [ -z "$CONTAINER" ] || ! docker inspect "$CONTAINER" >/dev/null 2>&1; then
  echo "The Compose bears-postgres service is not running. Set BEARS_POSTGRES_CONTAINER to inspect another stack." >&2
  exit 69
fi

psql_cmd() {
  docker exec -i "$CONTAINER" psql -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1 "$@"
}

TABLES_READY="$(psql_cmd -Atc "
  SELECT to_regclass('public.turn_runs') IS NOT NULL
     AND to_regclass('public.turn_obligations') IS NOT NULL
     AND to_regclass('public.turn_obligation_results') IS NOT NULL
     AND to_regclass('public.bearwire_events') IS NOT NULL
")"
if [ "$TABLES_READY" != "t" ]; then
  echo "Current BearWire lifecycle tables are not present in $DB_NAME." >&2
  echo "Available run/obligation/event tables:" >&2
  psql_cmd -c "
    SELECT schemaname, tablename
    FROM pg_tables
    WHERE tablename LIKE '%run%'
       OR tablename LIKE '%obligation%'
       OR tablename = 'bearwire_events'
    ORDER BY tablename
  " >&2 || true
  exit 69
fi

SESSION_ID="$(
  psql_cmd -v run_id="$RUN_ID" -At <<'SQL'
SELECT session_id FROM turn_runs WHERE run_id = :'run_id' LIMIT 1;
SQL
)"
if [ -z "$SESSION_ID" ]; then
  echo "No turn_runs row found for run_id '$RUN_ID'." >&2
  exit 66
fi

printf '# BearWire run\n'
psql_cmd -v run_id="$RUN_ID" -x <<'SQL'
SELECT run_id, session_id, state, terminal_reason, created_at, updated_at, completed_at
FROM turn_runs
WHERE run_id = :'run_id';
SQL

printf '\n# Obligations\n'
psql_cmd -v run_id="$RUN_ID" -x <<'SQL'
SELECT id, kind, expected_responder_action, tool_call_id, permission_id, state,
       request_payload->>'tool_name' AS tool_name,
       request_payload->>'execution_target' AS execution_target,
       result_payload->>'status' AS result_status,
       responder_ref_id, turn_step_id, claimed_at, lease_expires_at,
       created_at, updated_at, completed_at
FROM turn_obligations
WHERE run_id = :'run_id'
ORDER BY created_at, id;
SQL

printf '\n# Obligation results\n'
psql_cmd -v run_id="$RUN_ID" -x <<'SQL'
SELECT obligation_kind, obligation_id, result_hash, turn_step_id,
       payload_json->>'status' AS status,
       created_at
FROM turn_obligation_results
WHERE run_id = :'run_id'
ORDER BY created_at, id;
SQL

printf '\n# Event timeline\n'
psql_cmd -v run_id="$RUN_ID" -x <<'SQL'
SELECT sequence_no, event_type, event_json->>'run_id' AS run_id,
       event_json->>'subject' AS subject,
       event_json->'data'->'tool_call'->>'name' AS tool_name,
       event_json->'data'->>'status' AS status,
       event_json->'data'->>'reason' AS reason,
       event_json->'data'->'from'->>'phase' AS from_phase,
       event_json->'data'->'to'->>'phase' AS to_phase,
       event_json->'data'->>'state_version' AS state_version,
       created_at
FROM bearwire_events
WHERE event_json->>'run_id' = :'run_id'
  AND event_type NOT IN ('message.delta', 'message.reasoning.delta', 'run.progress')
ORDER BY sequence_no;
SQL
