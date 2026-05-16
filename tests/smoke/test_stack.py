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
API = os.environ.get("BEARS_API_URL", "").rstrip("/")
SEEDED_USERNAME = "alice"
SEEDED_PASSWORD = "Never deploy seed passwords."
SEEDED_BEAR_SLUG = "test-bear"
SEEDED_ACP_TOKEN = "bears_acp_smoke_known_token_for_dev_and_ci_only_000000000000"
LETTA = service_url("BEARS_LETTA_URL", "bears-letta", 8283)
LETTA_API_KEY = os.environ.get("LETTA_API_KEY") or os.environ.get(
    "LETTA_SERVER_PASS", "dev-placeholder"
)


def request_with_retries(method, url, **kwargs):
    session = kwargs.pop("session", requests)
    last_error = None
    for _ in range(20):
        try:
            response = session.request(method, url, **kwargs)
            if response.status_code < 500:
                return response
            last_error = AssertionError(
                f"{url} returned {response.status_code}: {response.text}"
            )
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


def test_api_health_when_enabled():
    if not API:
        return
    response = request_with_retries("GET", f"{API}/health", timeout=5)
    assert response.status_code == 200


def test_acp_requires_bearer_token_when_api_enabled():
    if not API:
        return
    response = request_with_retries(
        "POST",
        f"{API}/acp/bears/{SEEDED_BEAR_SLUG}/sessions/smoke-session/prompt",
        json={"message": "hello", "client": "zed"},
        timeout=5,
    )
    assert response.status_code in (401, 404), response.text
    if response.status_code == 401:
        assert "error_code" in response.text


def parse_sse_data(response):
    events = []
    for frame in response.text.split("\n\n"):
        for line in frame.splitlines():
            if not line.startswith("data:"):
                continue
            raw = line[len("data:") :].strip()
            if raw and raw != "[DONE]":
                try:
                    events.append(__import__("json").loads(raw))
                except Exception:
                    pass
    return events


def letta_headers():
    return {"Authorization": f"Bearer {LETTA_API_KEY}"}


def test_acp_pair_does_not_persist_runtime_context_in_letta_user_message():
    if not API:
        return
    marker = f"smoke-boundary-check-{int(time.time())}"
    session_id = f"smoke-boundary-{int(time.time())}"
    response = request_with_retries(
        "POST",
        f"{API}/acp/bears/{SEEDED_BEAR_SLUG}/sessions/{session_id}/prompt",
        json={
            "message": marker,
            "client": "zed",
            "client_context": {"cwd": "/workspace"},
        },
        headers={"Authorization": f"Bearer {SEEDED_ACP_TOKEN}"},
        timeout=60,
    )
    assert response.status_code == 200, response.text
    events = parse_sse_data(response)
    conversation_ids = [
        event.get("conversation_id")
        for event in events
        if event.get("type") == "conversation_resolved" and event.get("conversation_id")
    ]
    assert conversation_ids, response.text
    conversation_id = conversation_ids[-1]

    history = request_with_retries(
        "GET",
        f"{LETTA}/v1/conversations/{conversation_id}/messages?limit=20&order=desc",
        headers=letta_headers(),
        timeout=10,
    )
    assert history.status_code == 200, history.text
    body = history.json()
    raw_messages = (
        body
        if isinstance(body, list)
        else body.get("messages") or body.get("data") or []
    )
    user_texts = []
    for msg in raw_messages:
        inner = msg.get("message") if isinstance(msg.get("message"), dict) else msg
        message_type = (
            inner.get("message_type")
            or inner.get("type")
            or msg.get("message_type")
            or msg.get("type")
        )
        if message_type != "user_message":
            continue
        text = (
            inner.get("content")
            or inner.get("text")
            or msg.get("content")
            or msg.get("text")
        )
        if isinstance(text, str):
            user_texts.append(text)
    matching = [text for text in user_texts if marker in text]
    assert matching, (
        f"marker {marker!r} not found in raw Letta user messages: {user_texts!r}"
    )
    text = matching[0]
    assert text.strip() == marker
    forbidden = [
        "<system-reminder",
        "<system_reminder",
        "ACP workflow state",
        "AUTHORITATIVE WORKFLOW STATE",
        "Den workboard context",
        "Trusted ACP session mode this turn",
    ]
    for needle in forbidden:
        assert needle not in text


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
