-- The projected raw default already contains each Bear's current selected
-- model. Keep it for the older schema. Named alternatives, effort metadata and
-- hat bindings cannot be represented there and are intentionally lost on down.
-- Stop the newer application, downgrade with migration tooling that knows this
-- version, then start the older binary. Do not start it before downgrade:
-- its startup schema-version guard rejects this newer migration version.
-- Retain catalog capability enrichment (and subsequent live/operator edits):
-- older binaries tolerate these extra JSON keys, and stripping them could erase
-- authoritative catalog data imported after the upgrade.
DROP TRIGGER bear_model_configurations_refresh_default_projection ON bear_model_configurations;
DROP FUNCTION refresh_bear_default_model_configuration_projection();
DROP TRIGGER bears_route_inserted_default_model_configuration ON bears;
DROP FUNCTION route_inserted_bear_default_model_configuration();
DROP TRIGGER bears_project_default_model_configuration ON bears;
DROP FUNCTION project_bear_default_model_configuration();
DROP FUNCTION select_legacy_bear_model_configuration(UUID, TEXT, UUID);
DROP FUNCTION legacy_bear_model_catalog_handle(TEXT);

DROP INDEX bear_hats_model_configuration_id_idx;
ALTER TABLE bear_hats DROP CONSTRAINT bear_hats_model_configuration_fkey;
ALTER TABLE bear_hats DROP COLUMN model_configuration_id;
DROP INDEX bears_default_model_configuration_id_idx;
ALTER TABLE bears DROP CONSTRAINT bears_default_model_configuration_fkey;
ALTER TABLE bears DROP COLUMN default_model_configuration_id;
DROP TABLE bear_model_configurations;
