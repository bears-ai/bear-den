-- Phase 1: reusable agent templates for operator console (create bear from template).
-- Optional linkage on bears for audit / re-provision UX.

CREATE TABLE bear_templates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid (),
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    system_prompt TEXT NOT NULL DEFAULT '',
    default_model TEXT NULL,
    tools_enabled JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_bear_templates_slug ON bear_templates (slug);

ALTER TABLE bears
    ADD COLUMN IF NOT EXISTS source_template_id UUID NULL REFERENCES bear_templates (id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_bears_source_template_id ON bears (source_template_id);
