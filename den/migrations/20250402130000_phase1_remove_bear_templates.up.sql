-- Removes bear_agent_templates / template_id if an earlier revision of 20250401120000 created them.
-- Safe no-op on fresh installs that only ran phase1_bear_provisioning_fields.

ALTER TABLE bears DROP CONSTRAINT IF EXISTS bears_template_id_fkey;

ALTER TABLE bears DROP COLUMN IF EXISTS template_id;

DROP INDEX IF EXISTS idx_bears_template_id;

DROP TABLE IF EXISTS bear_agent_templates;

DROP INDEX IF EXISTS idx_bear_agent_templates_slug;
