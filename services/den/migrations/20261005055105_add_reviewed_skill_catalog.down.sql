ALTER TABLE bear_skill_proposals DROP COLUMN reviewed_by_user_id, DROP COLUMN proposed_by_user_id;
ALTER TABLE bear_skills_manifest DROP COLUMN attached_by_user_id, DROP COLUMN enabled, DROP COLUMN catalog_entry_id;
DROP TABLE skill_catalog_entries;
