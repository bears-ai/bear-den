-- Named configurations own model/effort state. Bindings are whole-configuration
-- overrides; NULL means inheritance. Catalog membership is validated by Den at
-- write/execution time, not an FK: removing a catalog entry must leave an
-- inspectable, repairable configuration rather than deleting it or falling back.
CREATE TABLE bear_model_configurations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (btrim(name) <> ''),
    model_handle TEXT NOT NULL CHECK (btrim(model_handle) <> ''),
    thinking_effort TEXT NULL CHECK (thinking_effort IN ('low', 'medium', 'high')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (bear_id, id)
);
CREATE UNIQUE INDEX bear_model_configurations_bear_name_unique
    ON bear_model_configurations (bear_id, lower(name));

ALTER TABLE bears ADD COLUMN default_model_configuration_id UUID NULL;
ALTER TABLE bears ADD CONSTRAINT bears_default_model_configuration_fkey
    FOREIGN KEY (id, default_model_configuration_id)
    REFERENCES bear_model_configurations (bear_id, id) ON DELETE RESTRICT;
CREATE INDEX bears_default_model_configuration_id_idx
    ON bears (default_model_configuration_id) WHERE default_model_configuration_id IS NOT NULL;

ALTER TABLE bear_hats ADD COLUMN model_configuration_id UUID NULL;
ALTER TABLE bear_hats ADD CONSTRAINT bear_hats_model_configuration_fkey
    FOREIGN KEY (bear_id, model_configuration_id)
    REFERENCES bear_model_configurations (bear_id, id) ON DELETE RESTRICT;
CREATE INDEX bear_hats_model_configuration_id_idx
    ON bear_hats (model_configuration_id) WHERE model_configuration_id IS NOT NULL;

-- Bootstrap missing capability metadata for the exact seeded registry handles.
-- den-llm's registry supplies identities, but has no reasoning-effort field.
-- These explicit capability facts are not inferred from names, tool support,
-- loop-control tiers or Responses support. o1-mini reasons but has no effort
-- control. Model capability references: https://developers.openai.com/api/docs/models
-- and https://developers.openai.com/api/docs/guides/reasoning .
-- Do not overwrite an existing live/operator value, including false or null;
-- subsequent catalog imports own their authoritative capability metadata.
WITH capabilities (handle, supports_reasoning_effort) AS (
    VALUES
        ('openai/gpt-5.5', true),
        ('openai/gpt-5.1', true),
        ('openai/gpt-5', true),
        ('openai/gpt-5-mini', true),
        ('openai/gpt-5-nano', true),
        ('openai/gpt-4.1', false),
        ('openai/gpt-4.1-mini', false),
        ('openai/gpt-4.1-nano', false),
        ('openai/gpt-4o', false),
        ('openai/gpt-4o-mini', false),
        ('openai/o4-mini', true),
        ('openai/o3', true),
        ('openai/o3-mini', true),
        ('openai/o1', true),
        ('openai/o1-mini', false)
)
UPDATE model_selection_options catalog
SET metadata_json = jsonb_set(catalog.metadata_json, '{supports_reasoning_effort}',
        to_jsonb(capabilities.supports_reasoning_effort)),
    updated_at = now()
FROM capabilities
WHERE catalog.handle = capabilities.handle
    AND jsonb_typeof(catalog.metadata_json) = 'object'
    AND NOT (catalog.metadata_json ? 'supports_reasoning_effort');

-- Preserve explicit historical defaults even if the model is no longer
-- selectable. Such configurations fail closed at execution until repaired.
-- Historical profile model settings are deliberately not promoted into hats.
WITH historical_defaults AS (
    SELECT id, created_at, updated_at,
        regexp_replace(default_model, '^[[:space:]]+|[[:space:]]+$', '', 'g') AS model_handle
    FROM bears WHERE default_model IS NOT NULL
)
INSERT INTO bear_model_configurations (bear_id, name, model_handle, created_at, updated_at)
SELECT id, 'Default', model_handle, created_at, updated_at
FROM historical_defaults WHERE model_handle <> '';
UPDATE bears b SET default_model_configuration_id = c.id, default_model = c.model_handle
FROM bear_model_configurations c WHERE c.bear_id = b.id AND c.name = 'Default';
UPDATE bears SET default_model = NULL WHERE default_model_configuration_id IS NULL;

-- Exact legacy aliases from den-llm::model_registry::registry_entries. This is
-- boundary normalization, not a model-name/provider capability heuristic.
-- Custom catalog handles retain their exact identity.
CREATE FUNCTION legacy_bear_model_catalog_handle(model TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    WITH requested AS (
        SELECT nullif(regexp_replace(model, '^[[:space:]]+|[[:space:]]+$', '', 'g'), '') AS handle
    )
    SELECT coalesce(known.handle, requested.handle)
    FROM requested
    LEFT JOIN (VALUES
        ('openai/gpt-5.5', 'gpt-5.5', 'openai:gpt-5.5'),
        ('openai/gpt-5.1', 'gpt-5.1', 'openai:gpt-5.1'),
        ('openai/gpt-5', 'gpt-5', 'openai:gpt-5'),
        ('openai/gpt-5-mini', 'gpt-5-mini', 'openai:gpt-5-mini'),
        ('openai/gpt-5-nano', 'gpt-5-nano', 'openai:gpt-5-nano'),
        ('openai/gpt-4.1', 'gpt-4.1', 'openai:gpt-4.1'),
        ('openai/gpt-4.1-mini', 'gpt-4.1-mini', 'openai:gpt-4.1-mini'),
        ('openai/gpt-4.1-nano', 'gpt-4.1-nano', 'openai:gpt-4.1-nano'),
        ('openai/gpt-4o', 'gpt-4o', 'openai:gpt-4o'),
        ('openai/gpt-4o-mini', 'gpt-4o-mini', 'openai:gpt-4o-mini'),
        ('openai/o4-mini', 'o4-mini', 'openai:o4-mini'),
        ('openai/o3', 'o3', 'openai:o3'),
        ('openai/o3-mini', 'o3-mini', 'openai:o3-mini'),
        ('openai/o1', 'o1', 'openai:o1'),
        ('openai/o1-mini', 'o1-mini', 'openai:o1-mini')
    ) AS known(handle, bare_alias, qualified_alias)
        ON requested.handle IN (known.handle, known.bare_alias, known.qualified_alias);
$$;

-- Legacy model-only edits never mutate a named configuration, including one
-- referenced by hats. Reuse a matching effort-free configuration or create a
-- uniquely named alternative. The Bear row lock serializes legacy edits.
CREATE FUNCTION select_legacy_bear_model_configuration(
    owner_bear_id UUID, model TEXT, current_configuration_id UUID
) RETURNS UUID
LANGUAGE plpgsql AS $$
DECLARE
    requested_model TEXT := nullif(regexp_replace(model, '^[[:space:]]+|[[:space:]]+$', '', 'g'), '');
    canonical_model TEXT := legacy_bear_model_catalog_handle(model);
    current_model TEXT;
    catalog_model TEXT;
    catalog_selectable BOOLEAN;
    selected_id UUID;
    selected_name TEXT;
BEGIN
    IF requested_model IS NULL THEN
        RETURN NULL;
    END IF;
    SELECT model_handle INTO current_model FROM bear_model_configurations
    WHERE bear_id = owner_bear_id AND id = current_configuration_id;
    IF current_model IS NOT NULL
        AND legacy_bear_model_catalog_handle(current_model) = canonical_model THEN
        -- An unchanged model (including an alias) is an unrelated Bear edit,
        -- not permission to erase effort or replace the selected config.
        RETURN current_configuration_id;
    END IF;
    IF requested_model = '*' OR requested_model LIKE '%/*' THEN
        RAISE EXCEPTION 'legacy default model must be a concrete Den catalog model'
            USING ERRCODE = '23514', CONSTRAINT = 'bears_legacy_default_model_selectable';
    END IF;
    SELECT handle, selectable INTO catalog_model, catalog_selectable
    FROM model_selection_options
    WHERE handle IN (canonical_model, requested_model)
    ORDER BY (handle = canonical_model) DESC LIMIT 1;
    IF NOT FOUND OR NOT catalog_selectable THEN
        RAISE EXCEPTION 'legacy default model must be a selectable Den catalog model: %', requested_model
            USING ERRCODE = '23514', CONSTRAINT = 'bears_legacy_default_model_selectable';
    END IF;
    SELECT id INTO selected_id FROM bear_model_configurations
    WHERE bear_id = owner_bear_id AND model_handle = catalog_model AND thinking_effort IS NULL
    ORDER BY created_at, id LIMIT 1 FOR KEY SHARE;
    IF selected_id IS NOT NULL THEN
        RETURN selected_id;
    END IF;
    selected_id := gen_random_uuid();
    IF EXISTS (SELECT 1 FROM bear_model_configurations
        WHERE bear_id = owner_bear_id AND lower(name) = 'default') THEN
        selected_name := 'Legacy default (' || selected_id::text || ')';
    ELSE
        selected_name := 'Default';
    END IF;
    -- INSERT only: no configuration UPDATE and therefore no projection-refresh
    -- recursion. Uniqueness races with modern naming edits fail atomically.
    INSERT INTO bear_model_configurations (id, bear_id, name, model_handle)
    VALUES (selected_id, owner_bear_id, selected_name, catalog_model);
    RETURN selected_id;
END;
$$;

-- Genuine expand compatibility: old serving binaries can keep writing the raw
-- model field. Translate a changed raw value into a canonical binding in the
-- same statement, then derive the stored raw value from that binding. A changed
-- canonical binding takes precedence over a simultaneously supplied raw value.
CREATE FUNCTION project_bear_default_model_configuration() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    projected_model TEXT;
BEGIN
    IF NEW.default_model_configuration_id IS NOT DISTINCT FROM OLD.default_model_configuration_id
        AND NEW.default_model IS DISTINCT FROM OLD.default_model THEN
        NEW.default_model_configuration_id := select_legacy_bear_model_configuration(
            NEW.id, NEW.default_model, OLD.default_model_configuration_id);
    END IF;
    SELECT model_handle INTO projected_model FROM bear_model_configurations
    WHERE bear_id = NEW.id AND id = NEW.default_model_configuration_id;
    NEW.default_model := projected_model;
    RETURN NEW;
END;
$$;
CREATE TRIGGER bears_project_default_model_configuration
    BEFORE UPDATE OF default_model, default_model_configuration_id ON bears
    FOR EACH ROW EXECUTE FUNCTION project_bear_default_model_configuration();

-- A configuration's owner must already exist to satisfy its FK, so legacy
-- INSERTs are routed AFTER insertion. The internal pointer UPDATE uses the
-- projection above; this trigger fires only on INSERT and cannot recurse.
CREATE FUNCTION route_inserted_bear_default_model_configuration() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    selected_id UUID;
BEGIN
    selected_id := select_legacy_bear_model_configuration(NEW.id, NEW.default_model, NULL);
    UPDATE bears SET default_model_configuration_id = selected_id WHERE id = NEW.id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER bears_route_inserted_default_model_configuration
    AFTER INSERT ON bears FOR EACH ROW WHEN (NEW.default_model IS NOT NULL)
    EXECUTE FUNCTION route_inserted_bear_default_model_configuration();

CREATE FUNCTION refresh_bear_default_model_configuration_projection() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- The raw field is unchanged in the input row, so this invokes projection,
    -- not legacy routing. The routing helper never updates configurations.
    UPDATE bears SET default_model_configuration_id = default_model_configuration_id,
        updated_at = now()
    WHERE id = NEW.bear_id AND default_model_configuration_id = NEW.id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER bear_model_configurations_refresh_default_projection
    AFTER UPDATE OF model_handle ON bear_model_configurations
    FOR EACH ROW EXECUTE FUNCTION refresh_bear_default_model_configuration_projection();
