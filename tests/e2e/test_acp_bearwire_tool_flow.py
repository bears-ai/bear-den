import json
import os
import subprocess
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from tests.support.armature_client import (
    ARMATURE_BIN,
    ROOT,
    ArmatureClient,
    build_armature,
)


class FakeBearWireState:
    def __init__(self, scenario="single"):
        self.lock = threading.Lock()
        self.events = []
        self.next_sequence = 1
        self.run_id = "run-e2e-tool-flow"
        self.session_id = None
        self.permission_result_payloads = []
        self.tool_result_payloads = []
        self.run_start_payloads = []
        self.scenario = scenario
        self.loop_tools = [
            ("fs_read_text_file", {"path": "docs/roadmap/MISSING.md", "limit": 120}),
            ("fs_list_directory", {"path": ".", "include_hidden": False, "limit": 20}),
            ("fs_find_paths", {"root": ".", "glob": "**/PLAN.md", "limit": 20}),
        ]

    def append_event(self, event):
        with self.lock:
            sequence = self.next_sequence
            self.next_sequence += 1
            event = dict(event)
            event.setdefault("sequence", sequence)
            event.setdefault("event_id", f"evt-e2e-{sequence}")
            self.events.append((sequence, event))
            return sequence

    def events_after(self, after):
        with self.lock:
            return [
                (seq, ev) for (seq, ev) in self.events if after is None or seq > after
            ]

    def append_waiting_for_tool(self, index):
        tool_name, args = self.loop_tools[index]
        permission_id = f"perm-loop-{index}"
        tool_call_id = f"call-loop-{index}"
        self.append_event(
            {
                "type": "client.waiting",
                "run_id": self.run_id,
                "session_id": self.session_id,
                "data": {
                    "obligation_id": f"obl-loop-{index}",
                    "expected_client_method": "client.permission.result",
                    "expected_responder_action": "permission_decision",
                    "permission_id": permission_id,
                    "tool_call_id": tool_call_id,
                    "tool_call": {
                        "id": tool_call_id,
                        "name": tool_name,
                        "title": tool_name,
                        "kind": "function",
                        "arguments": args,
                    },
                    "permission": {
                        "id": permission_id,
                        "reason": "e2e loop permission",
                    },
                },
            }
        )
        self.append_event(
            {
                "type": "run.paused",
                "run_id": self.run_id,
                "session_id": self.session_id,
                "data": {"reason": "requires_approval", "resume_token": permission_id},
            }
        )


class FakeBearWireHandler(BaseHTTPRequestHandler):
    state: FakeBearWireState

    def log_message(self, format, *args):
        return

    def _json(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _sse(self, frames):
        data = b"".join(frames)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        parsed = urllib.parse.urlparse(self.path)
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length).decode() if length else "{}"
        try:
            request = json.loads(body)
        except json.JSONDecodeError:
            self._json(400, {"error": "bad json"})
            return

        if parsed.path != "/bearwire/v1/rpc":
            self._json(404, {"error": "not found"})
            return

        method = request.get("method")
        req_id = request.get("id")
        params = request.get("params") or {}
        result = self.rpc_result(method, params)
        self._json(200, {"jsonrpc": "2.0", "id": req_id, "result": result})

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path == "/version.json":
            self._json(200, {"service": "den", "version": "e2e", "git_sha": "fake"})
            return
        if not parsed.path.startswith("/bearwire/v1/sessions/"):
            self._json(404, {"error": "not found"})
            return
        query = urllib.parse.parse_qs(parsed.query)
        after_values = query.get("after") or []
        after = int(after_values[0]) if after_values else None
        events = self.state.events_after(after)
        if parsed.path.endswith("/events/page"):
            next_after = events[-1][0] if events else after or 0
            self._json(
                200,
                {
                    "ok": True,
                    "session_id": self.state.session_id,
                    "events": [
                        {"sequence": sequence, "event": event}
                        for sequence, event in events
                    ],
                    "next_after": next_after,
                    "has_more": False,
                },
            )
            return
        if parsed.path.endswith("/events"):
            frames = []
            for sequence, event in events:
                payload = {"jsonrpc": "2.0", "method": "event", "params": event}
                frames.append(
                    f"id: {sequence}\ndata: {json.dumps(payload)}\n\n".encode()
                )
            self._sse(frames)
            return
        self._json(404, {"error": "not found"})

    def rpc_result(self, method, params):
        state = self.state
        if method == "initialize":
            return {
                "protocol": "bearwire",
                "version": 1,
                "server": {"name": "fake-den", "version": "e2e"},
            }
        if method == "session.state":
            return {"kind": "session_state", "ok": True}
        if method == "run.state":
            recent_events = [event for _, event in state.events_after(None)]
            terminal = next(
                (
                    event
                    for event in reversed(recent_events)
                    if event.get("type")
                    in {"run.completed", "run.failed", "run.cancelled"}
                ),
                None,
            )
            run_state = (
                terminal["type"].removeprefix("run.") if terminal else "running"
            )
            return {
                "kind": "run_state",
                "run": {
                    "run_id": state.run_id,
                    "session_id": state.session_id,
                    "state": run_state,
                    "terminal_reason": (terminal or {}).get("data", {}).get("reason"),
                },
                "open_obligations": [],
                "obligations": [],
                "recent_events": recent_events,
            }
        if method == "resource.update":
            return {"ok": True}
        if method == "session.model.get":
            return {"ok": True, "model": "openai/e2e-mock"}
        if method == "session.open":
            state.session_id = params.get("session_id")
            return {
                "ok": True,
                "event_sequence": 0,
                "session": {
                    "id": "fake-den-session",
                    "client_session_id": state.session_id,
                    "conversation_id": params.get("conversation_id") or "default",
                    "current_mode": params.get("mode") or "ask",
                },
            }
        if method == "run.start":
            state.run_start_payloads.append(params)
            state.session_id = params.get("session_id")
            if state.scenario == "loop":
                state.append_waiting_for_tool(0)
            else:
                state.append_event(
                    {
                        "type": "client.waiting",
                        "run_id": state.run_id,
                        "session_id": state.session_id,
                        "data": {
                            "obligation_id": "obl-e2e-1",
                            "expected_client_method": "client.permission.result",
                            "expected_responder_action": "permission_decision",
                            "permission_id": "perm-e2e-1",
                            "tool_call_id": "call-e2e-read",
                            "tool_call": {
                                "id": "call-e2e-read",
                                "name": "fs_read_text_file",
                                "title": "Read file",
                                "kind": "function",
                                "arguments": {
                                    "path": "docs/roadmap/PLAN.md",
                                    "limit": 120,
                                },
                            },
                            "permission": {
                                "id": "perm-e2e-1",
                                "reason": "read test file",
                            },
                        },
                    }
                )
                # This stale status event should not be surfaced as ordinary stderr noise.
                state.append_event(
                    {
                        "type": "run.paused",
                        "run_id": state.run_id,
                        "session_id": state.session_id,
                        "data": {
                            "reason": "requires_approval",
                            "resume_token": "perm-e2e-1",
                        },
                    }
                )
            return {"ok": True, "run_id": state.run_id, "event_sequence": 1}
        if method == "client.tool.claim":
            return {
                "ok": True,
                "status": "claimed",
                "attempt_token": f"attempt-{params.get('tool_call_id')}",
                "lease_expires_at": "2099-01-01T00:00:00Z",
                "renew_after_ms": 30_000,
            }
        if method == "client.tool.renew":
            return {
                "ok": True,
                "status": "renewed",
                "lease_expires_at": "2099-01-01T00:00:00Z",
                "renew_after_ms": 30_000,
            }
        if method == "client.permission.result":
            state.permission_result_payloads.append(params)
            if state.scenario == "loop":
                index = len(state.permission_result_payloads) - 1
                tool_name, args = state.loop_tools[index]
                return {
                    "ok": True,
                    "duplicate": False,
                    "event_sequence": state.next_sequence,
                    "run_state": "waiting_for_tool_result",
                    "continuation": "waiting_for_tool_result",
                    "obligation_state": "waiting_for_client",
                    "local_tool_request": {
                        "tool_call_id": f"call-loop-{index}",
                        "tool_name": tool_name,
                        "result_tool_name": tool_name,
                        "permission_id": f"perm-loop-{index}",
                        "obligation_id": f"obl-loop-{index}",
                        "args": args,
                    },
                }
            return {
                "ok": True,
                "duplicate": False,
                "event_sequence": state.next_sequence,
                "run_state": "waiting_for_tool_result",
                "continuation": "waiting_for_tool_result",
                "obligation_state": "waiting_for_client",
                "local_tool_request": {
                    "tool_call_id": "call-e2e-read",
                    "tool_name": "fs_read_text_file",
                    "result_tool_name": "fs_read_text_file",
                    "permission_id": "perm-e2e-1",
                    "obligation_id": "obl-e2e-1",
                    "args": {"path": "docs/roadmap/PLAN.md", "limit": 120},
                },
            }
        if method == "client.tool.result":
            state.tool_result_payloads.append(params)
            if state.scenario == "loop":
                index = len(state.tool_result_payloads) - 1
                expected_tool, _ = state.loop_tools[index]
                assert params.get("tool_call_id") == f"call-loop-{index}", params
                assert params.get("tool_name") == expected_tool, params
                if index + 1 < len(state.loop_tools):
                    state.append_waiting_for_tool(index + 1)
                else:
                    state.append_event(
                        {
                            "type": "run.failed",
                            "run_id": state.run_id,
                            "session_id": state.session_id,
                            "data": {
                                "reason": "max_agent_steps",
                                "message": "Tool budget exhausted before final answer in e2e loop.",
                                "run_id": state.run_id,
                            },
                        }
                    )
                return {
                    "ok": True,
                    "duplicate": False,
                    "event_sequence": state.next_sequence,
                    "run_state": "continuing",
                    "continuation": "started",
                    "result_id": f"result-loop-{index}",
                }
            content = (
                (params.get("structured_content") or {})
                if isinstance(params.get("structured_content"), dict)
                else {}
            )
            assert params.get("tool_call_id") == "call-e2e-read"
            assert params.get("tool_name") == "fs_read_text_file", params
            assert params.get("status") == "ok", params
            assert params.get("error") in (None, {}), params
            assert "e2e fixture plan" in json.dumps(content), params
            state.append_event(
                {
                    "type": "message.delta",
                    "run_id": state.run_id,
                    "session_id": state.session_id,
                    "data": {"delta": "Read the requested plan file successfully."},
                }
            )
            state.append_event(
                {
                    "type": "run.completed",
                    "run_id": state.run_id,
                    "session_id": state.session_id,
                    "data": {"outcome": "ok"},
                }
            )
            return {
                "ok": True,
                "duplicate": False,
                "event_sequence": state.next_sequence,
                "run_state": "continuing",
                "continuation": "started",
                "result_id": "result-e2e-1",
            }
        self._json(500, {"error": f"unexpected method {method}"})
        raise RuntimeError(method)



def start_fake_bearwire_server(scenario="single"):
    state = FakeBearWireState(scenario=scenario)
    handler = type("Handler", (FakeBearWireHandler,), {})
    handler.state = state
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, state, f"http://127.0.0.1:{server.server_address[1]}"


def test_acp_bearwire_relative_tool_flow(tmp_path):
    build_armature()
    workspace = tmp_path / "workspace"
    plan = workspace / "docs" / "roadmap" / "PLAN.md"
    plan.parent.mkdir(parents=True)
    plan.write_text(
        "# e2e fixture plan\n\nThis proves ACP relative paths resolve before tool use.\n"
    )

    server, state, api_url = start_fake_bearwire_server()
    env = os.environ.copy()
    env.update(
        {
            "DEN_API_URL": api_url,
            "BEAR_SLUG": "meta",
            "DEN_TOKEN": "bear_arm_e2e_fake_token",
            "DEN_ACP_CLIENT": "e2e-acp",
            "BEARS_BEARWIRE": "true",
            "BEAR_DEBUG": "off",
        }
    )
    proc = subprocess.Popen(
        [str(ARMATURE_BIN), "acp"],
        cwd=ROOT,
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
        init = client.wait_response(init_id)
        assert "result" in init, init

        new_id = client.send(
            "session/new",
            {
                "cwd": str(workspace),
                "workspace": {"roots": [{"rootUri": workspace.as_uri()}]},
            },
            "new",
        )
        new = client.wait_response(new_id)
        assert "result" in new, new
        session_id = new["result"]["sessionId"]

        prompt_id = client.send(
            "session/prompt",
            {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "Read the plan"}],
            },
            "prompt",
        )

        permission = client.wait_client_request(
            "session/request_permission", timeout=10
        )
        client.respond(
            permission, {"outcome": {"outcome": "selected", "optionId": "allow_once"}}
        )

        prompt = client.wait_response(prompt_id, timeout=15)
        assert "result" in prompt, prompt

        assert state.run_start_payloads, "run.start was not called"
        client_context = state.run_start_payloads[0].get("client_context") or {}
        assert client_context.get("workspace_roots") == [str(workspace)]
        assert state.permission_result_payloads, "permission result not posted"
        assert state.tool_result_payloads, "tool result not posted"
        assert state.tool_result_payloads[0]["status"] == "ok"
        completed_updates = [
            msg
            for msg in client.notifications
            if msg.get("method") == "session/update"
            and msg.get("params", {}).get("update", {}).get("sessionUpdate")
            == "tool_call"
            and msg.get("params", {}).get("update", {}).get("status") == "completed"
        ]
        assert completed_updates, client.notifications
        update = completed_updates[-1]["params"]["update"]
        raw_input = update.get("rawInput") or update.get("raw_input")
        raw_output = update.get("rawOutput") or update.get("raw_output")
        assert "docs/roadmap/PLAN.md" in json.dumps(raw_input), update
        visible_text = json.dumps(update.get("content", []))
        assert "e2e fixture plan" in visible_text, update
        assert raw_output, update

        stderr = "\n".join(client.stderr_lines)
        assert "BearWire run paused" not in stderr
        assert "JSON-RPC client request sent" not in stderr
        assert "permission_auto_allowed" not in stderr
        assert "Tool completed" not in stderr
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        server.shutdown()


def test_acp_bearwire_tool_loop_terminates_cleanly_without_stderr_noise(tmp_path):
    build_armature()
    workspace = tmp_path / "workspace"
    plan = workspace / "docs" / "roadmap" / "PLAN.md"
    plan.parent.mkdir(parents=True)
    plan.write_text("# e2e loop plan\n")

    server, state, api_url = start_fake_bearwire_server(scenario="loop")
    env = os.environ.copy()
    env.update(
        {
            "DEN_API_URL": api_url,
            "BEAR_SLUG": "meta",
            "DEN_TOKEN": "bear_arm_e2e_fake_token",
            "DEN_ACP_CLIENT": "e2e-acp",
            "BEARS_BEARWIRE": "true",
            "BEAR_DEBUG": "off",
        }
    )
    proc = subprocess.Popen(
        [str(ARMATURE_BIN), "acp"],
        cwd=ROOT,
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
            "initialize",
            {"clientCapabilities": {"fs": {"readTextFile": True}}},
            "init-loop",
        )
        assert "result" in client.wait_response(init_id)

        new_id = client.send(
            "session/new",
            {"cwd": str(workspace), "workspace": {"roots": [str(workspace)]}},
            "new-loop",
        )
        new = client.wait_response(new_id)
        session_id = new["result"]["sessionId"]

        prompt_id = client.send(
            "session/prompt",
            {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "Loop tools"}],
            },
            "prompt-loop",
        )

        deadline = time.time() + 20
        while time.time() < deadline and len(state.tool_result_payloads) < len(
            state.loop_tools
        ):
            if prompt_id in client.responses:
                break
            try:
                req = client.wait_any_client_request(timeout=0.25)
            except AssertionError:
                continue
            method = req.get("method")
            if method == "session/request_permission":
                client.respond(
                    req, {"outcome": {"outcome": "selected", "optionId": "allow_once"}}
                )
            else:
                pytest.fail(f"unexpected ACP client request: {req}")

        prompt = client.wait_response(prompt_id, timeout=10)
        assert "error" in prompt, prompt
        assert "Den could not complete this turn" in prompt["error"]["message"], prompt
        terminal = next(
            event
            for _, event in reversed(state.events_after(None))
            if event.get("type") == "run.failed"
        )
        assert terminal["data"]["reason"] == "max_agent_steps", terminal
        assert "Tool budget exhausted before final answer" in terminal["data"]["message"]
        assert len(state.permission_result_payloads) == len(state.loop_tools)
        assert len(state.tool_result_payloads) == len(state.loop_tools)
        assert state.tool_result_payloads[0]["status"] == "error"
        assert state.tool_result_payloads[0]["tool_name"] == "fs_read_text_file"
        assert state.tool_result_payloads[1]["status"] == "ok"
        assert state.tool_result_payloads[2]["status"] == "ok"

        stderr = "\n".join(client.stderr_lines)
        assert "continuation_start_failed" not in stderr
        assert "BearWire run paused" not in stderr
        assert "posted BearWire tool result" not in stderr
        assert "posted BearWire permission result" not in stderr
        assert "permission_auto_allowed" not in stderr
        assert "JSON-RPC client request sent" not in stderr
        assert "list_directory session_id" not in stderr
        assert "find_paths session_id" not in stderr
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        server.shutdown()
