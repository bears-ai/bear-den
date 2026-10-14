"""Validate the resolved Compose boundary without starting the application stack."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


def compose(*, enabled=True, development=False, **overrides):
    environment = dict(
        os.environ,
        COMPOSE_PROFILES="",
        BEARS_INSTANCE_SUFFIX="-validation",
        DEN_IMAGE="bears-den:compose-contract",
        DATABASE_URL="postgres://test:test@postgres.invalid/den",
        JWT_SECRET="compose-contract-test-only-secret",
        DEN_WEB_ORIGIN="https://den.example.test",
        DEN_API_ORIGIN="https://api.example.test",
        CABINET_URL="https://cabinet.example.test",
        SERVICE_URL_CABINET="",
        CABINET_PORT="3000",
        CABINET_DB_MAX_CONNECTIONS="3",
        S3_ENDPOINT="https://storage.example.test",
        S3_BUCKET="contract-test",
        S3_ACCESS_KEY_ID="contract-test-key",
        S3_SECRET_ACCESS_KEY="contract-test-secret",
    )
    environment.update(overrides)
    command = [
        "docker", "compose", "--env-file", "/dev/null",
        "-f", str(ROOT / "docker-compose.yaml"),
    ]
    if development:
        command += ["-f", str(ROOT / "docker-compose.dev.yaml")]
    if enabled:
        command += ["--profile", "cabinet"]
    command += ["config", "--format", "json"]
    result = subprocess.run(
        command, env=environment, capture_output=True, text=True, timeout=30,
        check=True,
    )
    return json.loads(result.stdout)["services"]


@unittest.skipUnless(shutil.which("docker"), "Docker Compose CLI is required")
class CabinetComposeTests(unittest.TestCase):
    def test_shared_release_database_identity_and_storage_have_one_owner(self):
        services = compose()
        den, cabinet, migrate = (
            services[name] for name in ("bears-den", "bears-cabinet", "bears-den-migrate")
        )
        self.assertEqual(den["image"], cabinet["image"])
        self.assertEqual(cabinet["image"], migrate["image"])
        for key in ("DATABASE_URL", "JWT_SECRET", "CABINET_URL"):
            self.assertEqual(den["environment"][key], cabinet["environment"][key])
            self.assertEqual(cabinet["environment"][key], migrate["environment"][key])
        for key in (
            "S3_ENDPOINT", "S3_BUCKET", "S3_REGION", "S3_ACCESS_KEY_ID",
            "S3_SECRET_ACCESS_KEY", "S3_PUBLIC_URL", "S3_FORCE_PATH_STYLE",
        ):
            self.assertEqual(den["environment"][key], cabinet["environment"][key])
        self.assertEqual(cabinet["environment"]["DB_MAX_CONNECTIONS"], "3")
        self.assertEqual(cabinet["environment"]["WEB_SERVER_URL"], "https://den.example.test")
        self.assertEqual(cabinet["hostname"], "bears-cabinet-validation")

    def test_cabinet_is_a_narrow_read_only_process_without_native_volumes(self):
        cabinet = compose()["bears-cabinet"]
        self.assertEqual(cabinet["command"], ["cabinet-serve"])
        self.assertEqual(cabinet["profiles"], ["cabinet"])
        self.assertTrue(cabinet["read_only"])
        self.assertFalse(cabinet.get("volumes"))
        self.assertEqual(cabinet["tmpfs"], ["/tmp:rw,noexec,nosuid,size=32m"])
        for key in ("RUN_WEB", "RUN_API", "RUN_WORKERS", "RUN_SANDBOX"):
            self.assertEqual(cabinet["environment"][key], "false")
        for key in ("BEAR_SQLITE_DATA_DIR", "DOCKER_HOST", "SANDBOX_WORKSPACES_DIR"):
            self.assertNotIn(key, cabinet["environment"])
        self.assertEqual(cabinet["expose"], ["3000"])
        self.assertNotIn("ports", cabinet)
        self.assertIn("/health/ready", cabinet["healthcheck"]["test"][1])

    def test_migration_then_cabinet_health_precedes_den_handoffs(self):
        services = compose()
        cabinet = services["bears-cabinet"]
        self.assertEqual(set(cabinet["depends_on"]), {"bears-den-migrate"})
        self.assertEqual(
            cabinet["depends_on"]["bears-den-migrate"]["condition"],
            "service_completed_successfully",
        )
        dependency = services["bears-den"]["depends_on"]["bears-cabinet"]
        self.assertEqual(dependency["condition"], "service_healthy")
        self.assertFalse(dependency["required"])
        self.assertNotIn("bears-den", services["bears-den-migrate"]["depends_on"])

    def test_unconfigured_default_stack_preserves_local_cabinet(self):
        services = compose(enabled=False, CABINET_URL="", SERVICE_URL_CABINET="")
        self.assertNotIn("bears-cabinet", services)
        self.assertEqual(services["bears-den"]["environment"]["CABINET_URL"], "")
        # Compose retains the inactive dependency but must not require its service.
        self.assertFalse(services["bears-den"]["depends_on"]["bears-cabinet"]["required"])

    def test_platform_origin_fallback_and_custom_port_are_consistent(self):
        services = compose(
            CABINET_URL="", SERVICE_URL_CABINET="https://platform-cabinet.example.test",
            CABINET_PORT="3100",
        )
        for name in ("bears-den", "bears-cabinet", "bears-den-migrate"):
            self.assertEqual(
                services[name]["environment"]["CABINET_URL"],
                "https://platform-cabinet.example.test",
            )
        self.assertEqual(services["bears-cabinet"]["environment"]["PORT"], "3100")
        self.assertEqual(services["bears-cabinet"]["expose"], ["3100"])

    def test_cabinet_url_with_or_without_trailing_slash_reaches_every_composition(self):
        for url in ("https://cabinet.example.test", "https://cabinet.example.test/"):
            with self.subTest(url=url):
                services = compose(CABINET_URL=url)
                for name in ("bears-den", "bears-cabinet", "bears-den-migrate"):
                    self.assertEqual(services[name]["environment"]["CABINET_URL"], url)

    def test_development_override_reuses_the_den_build_and_never_pulls(self):
        services = compose(development=True)
        for name in ("bears-den", "bears-cabinet", "bears-den-migrate"):
            self.assertEqual(services[name]["pull_policy"], "never")
            self.assertEqual(services[name]["build"], services["bears-den"]["build"])


if __name__ == "__main__":
    unittest.main()
