DROP INDEX IF EXISTS idx_reflection_conversations_bear_lane_used;
DROP TABLE IF EXISTS reflection_conversations;

DROP INDEX IF EXISTS idx_bear_reflection_run_items_kind_item;
DROP INDEX IF EXISTS idx_bear_reflection_run_items_run_created;
DROP TABLE IF EXISTS bear_reflection_run_items;

DROP INDEX IF EXISTS idx_bear_reflection_runs_bear_conversation_date;
DROP INDEX IF EXISTS idx_bear_reflection_runs_bear_lane_status_created;
DROP TABLE IF EXISTS bear_reflection_runs;
