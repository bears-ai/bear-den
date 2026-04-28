import os
import socket
import subprocess

import requests


def service_url(env_name, service_name, port):
    override = os.environ.get(env_name)
    if override:
        return override.rstrip("/")
    try:
        socket.gethostbyname(service_name)
        host = service_name
    except OSError:
        container_id = subprocess.check_output(
            ["docker", "compose", "ps", "-q", service_name],
            text=True,
            timeout=5,
        ).strip()
        host = subprocess.check_output(
            [
                "docker",
                "inspect",
                "-f",
                "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
                container_id,
            ],
            text=True,
            timeout=5,
        ).strip()
    return f"http://{host}:{port}"


DEN = service_url("BEARS_DEN_URL", "bears-den", 3000)
MEMFS_MANAGER = service_url("BEARS_MEMFS_MANAGER_URL", "bears-memfs-manager", 8285)
CODEPOOL = service_url("BEARS_CODEPOOL_URL", "bears-codepool", 3030)
SEEDED_USERNAME = "alice"
SEEDED_PASSWORD = "Never deploy seed passwords."
SEEDED_BEAR_SLUG = "test-bear"


def test_memfs_manager_health():
    response = requests.get(f"{MEMFS_MANAGER}/health", timeout=5)
    assert response.status_code == 200


def test_den_reachable():
    response = requests.get(f"{DEN}/health", timeout=5)
    assert response.status_code == 200


def test_pool_health():
    response = requests.get(f"{CODEPOOL}/health", timeout=5)
    assert response.status_code == 200


def test_seeded_user_can_open_seeded_bear_page():
    session = requests.Session()
    login = session.post(
        f"{DEN}/login/password",
        data={"username": SEEDED_USERNAME, "password": SEEDED_PASSWORD},
        timeout=5,
        allow_redirects=False,
    )
    assert login.status_code in (302, 303), login.text

    response = session.get(f"{DEN}/bear/{SEEDED_BEAR_SLUG}", timeout=5)
    assert response.status_code == 200, response.text
    assert "Test Bear" in response.text
