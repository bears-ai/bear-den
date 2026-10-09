-- One external-reference-only repository operation; no backend or credential migration.
ALTER TABLE provider_connections
    ADD COLUMN external_backend_binding_id UUID,
    ADD COLUMN external_secret_id UUID,
    ADD COLUMN external_secret_version BIGINT;
ALTER TABLE provider_connections DROP CONSTRAINT provider_connections_provider_check;
ALTER TABLE provider_connections ADD CONSTRAINT provider_connections_provider_check
    CHECK (provider IN ('git_https', 'git_ssh', 'github_app', 'github_external'));
ALTER TABLE provider_connections DROP CONSTRAINT provider_connections_material;
ALTER TABLE provider_connections ADD CONSTRAINT provider_connections_material CHECK (
    (provider = 'github_external' AND secret_ciphertext IS NULL
        AND github_app_installation_id IS NULL AND NOT github_app_write_enabled
        AND external_backend_binding_id IS NOT NULL AND external_backend_binding_id <> '00000000-0000-0000-0000-000000000000'::uuid
        AND external_secret_id IS NOT NULL AND external_secret_id <> '00000000-0000-0000-0000-000000000000'::uuid
        AND external_secret_version IS NOT NULL AND external_secret_version > 0)
    OR (provider <> 'github_external' AND external_backend_binding_id IS NULL
        AND external_secret_id IS NULL AND external_secret_version IS NULL AND (
        (provider = 'github_app' AND github_app_installation_id IS NOT NULL AND github_app_installation_id > 0 AND secret_ciphertext IS NULL)
        OR (provider IN ('git_https', 'git_ssh') AND secret_ciphertext IS NOT NULL AND github_app_installation_id IS NULL)))
);
ALTER TABLE bear_hat_access_grants DROP CONSTRAINT bear_hat_access_grants_check;
ALTER TABLE bear_hat_access_grants ADD CONSTRAINT bear_hat_access_grants_check CHECK (
    (kind = 'tool' AND target_kind IN ('hat', 'workspace', 'directory', 'command_exact_workspace', 'command_family_workspace'))
    OR (kind = 'tool' AND action_key = 'den.repository.head' AND target_kind = 'repository')
    OR (kind = 'network' AND action_key = 'https' AND target_kind = 'host')
);
ALTER TABLE bear_hat_access_grants ADD CONSTRAINT bear_hat_repository_target CHECK (
    target_kind <> 'repository' OR target_value ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:[0-9a-f]{64}$'
);
