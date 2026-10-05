ALTER TABLE bear_skills_manifest DROP CONSTRAINT bear_skills_manifest_live_contexts;
ALTER TABLE bear_skills_manifest ADD CONSTRAINT bear_skills_manifest_applies_to_roles_check1 CHECK(applies_to_profiles <@ ARRAY['talk','pair','curate','work','watch']::TEXT[]) NOT VALID;
ALTER TABLE provider_connections DROP CONSTRAINT provider_connections_material;
ALTER TABLE provider_connections ADD CONSTRAINT provider_connections_check CHECK((provider='github_app' AND github_app_installation_id>0 AND secret_ciphertext IS NULL) OR (provider IN('git_https','git_ssh') AND secret_ciphertext IS NOT NULL AND github_app_installation_id IS NULL));
