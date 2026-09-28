DROP TRIGGER IF EXISTS client_sessions_global_owner_guard ON client_sessions;
DROP FUNCTION IF EXISTS guard_client_session_id_ownership();
