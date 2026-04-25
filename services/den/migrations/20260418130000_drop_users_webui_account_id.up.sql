-- BEARS uses Den embedded chat only; no external web UI account mapping column.
DROP INDEX IF EXISTS users_webui_account_id_key;
ALTER TABLE users DROP COLUMN IF EXISTS webui_account_id;
