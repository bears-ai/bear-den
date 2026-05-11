-- Managed prompt/config blocks foundation: system-owned versioned blocks and per-Bear bindings.

CREATE TABLE IF NOT EXISTS system_blocks (
    key TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('prompt_text', 'tool_instruction', 'policy_text')),
    scope TEXT NOT NULL CHECK (scope IN ('global', 'space', 'template')),
    status TEXT NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published', 'archived')),
    current_published_version_id BIGINT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS system_block_versions (
    id BIGSERIAL PRIMARY KEY,
    block_key TEXT NOT NULL REFERENCES system_blocks (key) ON DELETE CASCADE,
    version_number INTEGER NOT NULL CHECK (version_number > 0),
    content TEXT NOT NULL,
    change_summary TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (block_key, version_number),
    UNIQUE (block_key, content_hash)
);

ALTER TABLE system_blocks
    ADD CONSTRAINT system_blocks_current_published_version_fk
    FOREIGN KEY (current_published_version_id)
    REFERENCES system_block_versions (id)
    ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_system_blocks_status ON system_blocks (status);
CREATE INDEX IF NOT EXISTS idx_system_block_versions_block_key ON system_block_versions (block_key, version_number DESC);

CREATE TABLE IF NOT EXISTS bear_block_bindings (
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    block_key TEXT NOT NULL REFERENCES system_blocks (key) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('inherit', 'custom')),
    custom_content TEXT NULL,
    forked_from_version_id BIGINT NULL REFERENCES system_block_versions (id) ON DELETE SET NULL,
    last_reviewed_version_id BIGINT NULL REFERENCES system_block_versions (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (bear_id, block_key),
    CHECK (
        (mode = 'inherit' AND custom_content IS NULL)
        OR
        (mode = 'custom' AND custom_content IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_bear_block_bindings_bear_id ON bear_block_bindings (bear_id);
CREATE INDEX IF NOT EXISTS idx_bear_block_bindings_block_key ON bear_block_bindings (block_key);

CREATE TABLE IF NOT EXISTS bear_compiled_configs (
    bear_id UUID PRIMARY KEY REFERENCES bears (id) ON DELETE CASCADE,
    compiled_version INTEGER NOT NULL DEFAULT 1 CHECK (compiled_version > 0),
    resolved_blocks_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    rendered_prompts_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    rendered_prompt_hashes_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    tool_guidance_hashes_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    config_hash TEXT NOT NULL,
    compiled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE system_blocks IS 'Den-managed semantic prompt/config blocks with published-version indirection.';
COMMENT ON TABLE system_block_versions IS 'Immutable content snapshots for managed system blocks.';
COMMENT ON TABLE bear_block_bindings IS 'Per-Bear inheritance/customization bindings for managed prompt/config blocks.';
COMMENT ON TABLE bear_compiled_configs IS 'Compiled effective Bear prompt/config artifacts resolved from managed blocks and Bear-local context.';
COMMENT ON COLUMN system_blocks.current_published_version_id IS 'Published system_block_versions.id used by inherited Bears.';
COMMENT ON COLUMN bear_block_bindings.mode IS 'inherit = use current published block version; custom = use custom_content.';
COMMENT ON COLUMN bear_compiled_configs.config_hash IS 'Deterministic hash over compiled effective prompt/config content for drift and reprovision checks.';
