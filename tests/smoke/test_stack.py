import os
import socket
import subprocess
import time

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


def request_with_retries(method, url, **kwargs):
    session = kwargs.pop("session", requests)
    last_error = None
    for _ in range(20):
        try:
            response = session.request(method, url, **kwargs)
            if response.status_code < 500:
                return response
            last_error = AssertionError(f"{url} returned {response.status_code}: {response.text}")
        except requests.RequestException as exc:
            last_error = exc
        time.sleep(2)
    raise AssertionError(f"request failed after retries: {url}: {last_error}")


def test_memfs_manager_health():
    response = request_with_retries("GET", f"{MEMFS_MANAGER}/health", timeout=5)
    assert response.status_code == 200


def test_den_reachable():
    response = request_with_retries("GET", f"{DEN}/health", timeout=5)
    assert response.status_code == 200


def test_pool_health():
    response = request_with_retries("GET", f"{CODEPOOL}/health", timeout=5)
    assert response.status_code == 200


def test_seeded_user_can_open_seeded_bear_page():
    session = requests.Session()
    login = request_with_retries(
        "POST",
        f"{DEN}/login/password",
        session=session,
        data={"username": SEEDED_USERNAME, "password": SEEDED_PASSWORD},
        timeout=5,
        allow_redirects=False,
    )
    assert login.status_code in (302, 303), login.text

    response = session.get(f"{DEN}/bear/{SEEDED_BEAR_SLUG}", timeout=5)
    assert response.status_code == 200, response.text
    assert "Test Bear" in response.text
