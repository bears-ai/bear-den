ALTER TABLE bear_skills_manifest DROP CONSTRAINT IF EXISTS bear_skills_manifest_applies_to_roles_check1;
ALTER TABLE bear_skills_manifest ADD CONSTRAINT bear_skills_manifest_live_contexts CHECK(cardinality(applies_to_profiles)>0 AND applies_to_profiles <@ ARRAY['chat','pair','curate','work','watch']::TEXT[]) NOT VALID;
ALTER TABLE provider_connections DROP CONSTRAINT provider_connections_check;
ALTER TABLE provider_connections ADD CONSTRAINT provider_connections_material CHECK((provider='github_app' AND github_app_installation_id IS NOT NULL AND github_app_installation_id>0 AND secret_ciphertext IS NULL) OR (provider IN('git_https','git_ssh') AND secret_ciphertext IS NOT NULL AND github_app_installation_id IS NULL));
