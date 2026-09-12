import os
import socket
import subprocess
import time
import uuid

import pytest
import requests

from tests.e2e.test_acp_bearwire_tool_flow import ARMATURE_BIN, ArmatureClient


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
API = os.environ.get("BEARS_API_URL", "").rstrip("/")
SEEDED_USERNAME = "alice"
SEEDED_PASSWORD = "Never deploy seed passwords."
SEEDED_BEAR_SLUG = "test-bear"
SEEDED_ARMATURE_TOKEN = "bear_arm_smoke_known_token_for_dev_and_ci_only_000000000000"
PLACEHOLDER_SECRETS = {"", "dev-placeholder", "SETME"}
TERMINAL_EVENTS = {"run.completed", "run.failed", "run.cancelled", "run.blocked"}


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


def seeded_user_session():
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
    return session


def bearwire_headers():
    return {
        "Authorization": f"Bearer {SEEDED_ARMATURE_TOKEN}",
        "BearWire-Version": "1",
    }


def bearwire_rpc(method, params, *, authenticated=True, timeout=30):
    response = request_with_retries(
        "POST",
        f"{API}/bearwire/v1/rpc",
        headers=bearwire_headers() if authenticated else {"BearWire-Version": "1"},
        json={
            "jsonrpc": "2.0",
            "id": f"smoke-{uuid.uuid4()}",
            "method": method,
            "params": {"bear_slug": SEEDED_BEAR_SLUG, **params},
        },
        timeout=timeout,
    )
    assert response.status_code == 200, response.text
    body = response.json()
    if authenticated:
        assert "error" not in body, body
    return body


def bearwire_events(session_id, after):
    response = request_with_retries(
        "GET",
        f"{API}/bearwire/v1/sessions/{session_id}/events/page",
        headers=bearwire_headers(),
        params={"bear_slug": SEEDED_BEAR_SLUG, "after": after, "limit": 100},
        timeout=30,
    )
    assert response.status_code == 200, response.text
    body = response.json()
    events = []
    for item in body.get("events", []):
        event = dict(item.get("event", item))
        event["_sequence"] = item.get("sequence", event.get("sequence"))
        events.append(event)
    return events, body.get("next_after", after)


def wait_for_run_terminal(session_id, run_id, timeout=120):
    deadline = time.time() + timeout
    after = 0
    events = []
    while time.time() < deadline:
        page, after = bearwire_events(session_id, after)
        events.extend(page)
        terminal = [
            event
            for event in events
            if event.get("run_id") == run_id and event.get("type") in TERMINAL_EVENTS
        ]
        if terminal:
            return events, terminal
        time.sleep(0.25)
    pytest.fail(
        f"run {run_id} did not terminate; last events="
        f"{[(event.get('_sequence'), event.get('type'), event.get('run_id')) for event in events[-30:]]}"
    )


def tool_names(events, run_id):
    names = []
    for event in events:
        if event.get("run_id") != run_id or event.get("type") != "tool_call.requested":
            continue
        data = event.get("data") or {}
        tool_call = data.get("tool_call") or {}
        name = tool_call.get("name") or data.get("tool_name")
        if name:
            names.append(name)
    return names


def test_native_stack_and_seeded_bear_are_ready():
    health = request_with_retries("GET", f"{DEN}/health/ready", timeout=5)
    assert health.status_code == 200, health.text

    session = seeded_user_session()
    landing = session.get(f"{DEN}/bear/{SEEDED_BEAR_SLUG}", timeout=10)
    assert landing.status_code == 200, landing.text[:400]
    assert "Test Bear" in landing.text

    for path in ("overview", "profiles", "memory"):
        response = session.get(f"{DEN}/bear/{SEEDED_BEAR_SLUG}/{path}", timeout=10)
        assert response.status_code == 200, f"{path}: {response.text[:400]}"


def test_bearwire_rejects_unauthenticated_session_open():
    if not API:
        pytest.skip("Den API service is disabled")
    body = bearwire_rpc(
        "session.open",
        {"session_id": f"smoke-unauth-{uuid.uuid4().hex}"},
        authenticated=False,
    )
    assert body["error"]["code"] == -32001, body
    assert "Authorization" in body["error"]["data"]["error"], body


def test_live_bearwire_pair_stance_turn_has_one_clean_terminal():
    if not API:
        pytest.skip("Den API service is disabled")
    if os.environ.get("OPENAI_API_KEY", "").strip() in PLACEHOLDER_SECRETS:
        pytest.skip("No live OpenAI key is configured")

    session_id = f"smoke-live-{uuid.uuid4().hex}"
    conversation_id = f"new-smoke-live-{uuid.uuid4()}"
    marker = f"smoke-live-ok-{uuid.uuid4().hex[:8]}"
    client_context = {"cwd": "/workspace", "tools": []}

    opened = bearwire_rpc(
        "session.open",
        {
            "session_id": session_id,
            "client": "smoke",
            "conversation_id": conversation_id,
            "cwd": "/workspace",
            "mode": "ask",
            "client_context": client_context,
        },
    )["result"]
    assert opened["ok"] is True, opened

    started = bearwire_rpc(
        "run.start",
        {
            "session_id": session_id,
            "client": "smoke",
            "conversation_id": conversation_id,
            "cwd": "/workspace",
            "requested_mode": "ask",
            "prompt": f"Reply with exactly: {marker}",
            "client_context": client_context,
        },
    )["result"]
    assert started["accepted"] is True, started
    run_id = started["run_id"]

    events, terminal = wait_for_run_terminal(session_id, run_id)
    assert len(terminal) == 1, terminal
    assert terminal[0]["type"] == "run.completed", terminal[0]
    assert not [
        event
        for event in events
        if event.get("run_id") == run_id and event.get("type") == "tool_call.requested"
    ], events

    assistant_text = "".join(
        (event.get("data") or {}).get("delta", "")
        for event in events
        if event.get("run_id") == run_id and event.get("type") == "message.delta"
    )
    assert marker in assistant_text, assistant_text

    state = bearwire_rpc(
        "run.state",
        {"session_id": session_id, "run_id": run_id, "limit": 100},
    )["result"]
    assert state["run"]["state"] == "completed", state
    assert state["open_obligations"] == [], state




def test_live_armature_acp_focus_flow_has_one_terminal_response():
    if not API:
        pytest.skip("Den API service is disabled")
    if os.environ.get("OPENAI_API_KEY", "").strip() in PLACEHOLDER_SECRETS:
        pytest.skip("No live OpenAI key is configured")

    subprocess.run(
        [
            "cargo",
            "build",
            "--manifest-path",
            "tools/bear-armature/Cargo.toml",
        ],
        cwd="/workspace",
        check=True,
        timeout=300,
    )
    marker = f"smoke-acp-focus-ok-{uuid.uuid4().hex[:8]}"
    env = os.environ.copy()
    env.update(
        {
            "DEN_API_URL": API,
            "BEAR_SLUG": SEEDED_BEAR_SLUG,
            "DEN_TOKEN": SEEDED_ARMATURE_TOKEN,
            "DEN_ACP_CLIENT": "smoke-acp",
            "BEARS_BEARWIRE": "true",
            "BEAR_DEBUG": "off",
        }
    )
    proc = subprocess.Popen(
        [str(ARMATURE_BIN), "acp"],
        cwd="/workspace",
        env=env,
        text=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=1,
    )
    client = ArmatureClient(proc)
    try:
        init_id = client.send(
            "initialize", {"clientCapabilities": {"fs": {"readTextFile": True}}}, "init"
        )
        assert "result" in client.wait_response(init_id, timeout=20)

        new_id = client.send(
            "session/new",
            {
                "cwd": "/workspace",
                "workspace": {"roots": [{"rootUri": "file:///workspace"}]},
            },
            "new",
        )
        opened = client.wait_response(new_id, timeout=30)
        assert "result" in opened, opened
        session_id = opened["result"]["sessionId"]

        mode_id = client.send(
            "session/set_mode",
            {"sessionId": session_id, "modeId": "write"},
            "mode",
        )
        assert "result" in client.wait_response(mode_id, timeout=30)
        model_id = client.send(
            "session/set_config_option",
            {
                "sessionId": session_id,
                "configId": "model",
                "value": "openai/gpt-5.5",
            },
            "model",
        )
        assert "result" in client.wait_response(model_id, timeout=30)

        prompt = f"""
Create one session-owned execution task titled "{marker}" with body
"Read README.md through the client tool and report success", and completion criteria
["README.md was read through fs_read_text_file", "Report {marker}"]. Then call
get_task_list_status, select_current_task with that new task's exact UUID, and
focus_current_task to promote this same run into focused execution. In focused execution call
fs_read_text_file for README.md exactly once. Then settle the new task by calling
update_current_task_status with its exact task UUID, status "done", outcome_disposition
"completed", and result_summary "{marker}". Finally reply with exactly: {marker}. Do not create
a Job, dispatch work, call any other tools, or start another run.
""".strip()
        prompt_id = client.send(
            "session/prompt",
            {"sessionId": session_id, "prompt": [{"type": "text", "text": prompt}]},
            "prompt",
        )

        fs_requests = []
        deadline = time.time() + 180
        while time.time() < deadline and prompt_id not in client.responses:
            try:
                request = client.wait_any_client_request(timeout=1)
            except AssertionError:
                continue
            method = request.get("method")
            if method == "session/request_permission":
                client.respond(
                    request,
                    {"outcome": {"outcome": "selected", "optionId": "allow_once"}},
                )
            elif method == "fs/read_text_file":
                fs_requests.append(request)
                client.respond(
                    request,
                    {"content": "# Smoke fixture\n\nThe focused ACP tool completed."},
                )
            else:
                pytest.fail(f"unexpected ACP client request: {request}")

        response = client.wait_response(prompt_id, timeout=10)
        assert "result" in response, {"response": response, "stderr": client.stderr_lines}
        assert len(fs_requests) <= 1, fs_requests
        time.sleep(0.5)
        assert prompt_id not in client.responses, "duplicate terminal ACP response"
        assert client.client_requests.empty(), "ACP client request remained unanswered"

        events, _ = bearwire_events(session_id, 0)
        focus_transitions = [
            event
            for event in events
            if event.get("type") == "diagnostic.state_transition"
            and (event.get("data") or {}).get("reason") == "focus_acquired"
        ]
        assert len(focus_transitions) == 1, focus_transitions
        run_id = focus_transitions[0]["run_id"]
        terminal = [
            event
            for event in events
            if event.get("run_id") == run_id and event.get("type") in TERMINAL_EVENTS
        ]
        assert [event["type"] for event in terminal] == ["run.completed"], terminal

        names = tool_names(events, run_id)
        assert names[:5] == [
            "create_task",
            "get_task_list_status",
            "select_current_task",
            "focus_current_task",
            "fs_read_text_file",
        ], names
        assert "update_current_task_status" in names, names
        assert len(names) <= 8, names

        state = bearwire_rpc(
            "run.state",
            {"session_id": session_id, "run_id": run_id, "limit": 200},
        )["result"]
        assert state["run"]["state"] == "completed", state
        assert state["open_obligations"] == [], state
        client_obligations = [
            obligation
            for obligation in state["obligations"]
            if obligation.get("tool_call_id")
        ]
        assert len(client_obligations) == 1, state
        assert client_obligations[0]["state"] == "continued", state

        diagnostics = bearwire_rpc(
            "session.execution.diagnostics",
            {"session_id": session_id, "limit": 100},
        )["result"]["diagnostics"]
        assert diagnostics["version_gap"] is False, diagnostics
        assert diagnostics["snapshot_matches_latest_transition"] is True, diagnostics
        assert any(
            record["transition"].get("run_id") == run_id
            for record in diagnostics["transitions"]
        ), diagnostics

        completed_cards = [
            message
            for message in client.notifications
            if message.get("method") == "session/update"
            and message.get("params", {}).get("update", {}).get("sessionUpdate")
            == "tool_call"
            and message.get("params", {}).get("update", {}).get("status") == "completed"
        ]
        assert completed_cards, client.notifications
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
