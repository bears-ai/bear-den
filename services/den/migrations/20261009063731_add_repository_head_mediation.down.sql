-- Explicit retirement is required; rollback must not turn references into legacy credentials.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM provider_connections WHERE provider = 'github_external')
        OR EXISTS (SELECT 1 FROM bear_hat_access_grants WHERE target_kind = 'repository') THEN
        RAISE EXCEPTION 'Retire external Connections and repository grant records explicitly before rollback';
    END IF;
END $$;
ALTER TABLE bear_hat_access_grants DROP CONSTRAINT bear_hat_repository_target;
ALTER TABLE bear_hat_access_grants DROP CONSTRAINT bear_hat_access_grants_check;
ALTER TABLE bear_hat_access_grants ADD CONSTRAINT bear_hat_access_grants_check CHECK (
    (kind = 'tool' AND target_kind IN ('hat', 'workspace', 'directory', 'command_exact_workspace', 'command_family_workspace'))
    OR (kind = 'network' AND action_key = 'https' AND target_kind = 'host')
);
ALTER TABLE provider_connections DROP CONSTRAINT provider_connections_material;
ALTER TABLE provider_connections DROP CONSTRAINT provider_connections_provider_check;
ALTER TABLE provider_connections ADD CONSTRAINT provider_connections_provider_check CHECK (provider IN ('git_https', 'git_ssh', 'github_app'));
ALTER TABLE provider_connections ADD CONSTRAINT provider_connections_material CHECK (
    (provider = 'github_app' AND github_app_installation_id IS NOT NULL AND github_app_installation_id > 0 AND secret_ciphertext IS NULL)
    OR (provider IN ('git_https', 'git_ssh') AND secret_ciphertext IS NOT NULL AND github_app_installation_id IS NULL)
);
ALTER TABLE provider_connections DROP COLUMN external_backend_binding_id, DROP COLUMN external_secret_id, DROP COLUMN external_secret_version;
