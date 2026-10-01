"""Run the packaged relay without a sandbox or live outbound network."""

import json
import os
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


RELAY = Path(__file__).resolve().parents[2] / "packaging/sandbox-image/egress-relay.sh"
RUN_ID = "11111111-1111-1111-1111-111111111111"


class DenDecision(BaseHTTPRequestHandler):
    allowed = False
    requests = []

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length))
        self.requests.append((self.path, self.headers.get("Authorization"), body))
        response = json.dumps({"jsonrpc": "2.0", "id": "egress", "result": {"allowed": self.allowed}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *_args):
        pass


@pytest.mark.parametrize("allowed", [True, False])
def test_relay_checks_den_before_dialling_pinned_address(tmp_path, allowed):
    DenDecision.allowed = allowed
    DenDecision.requests = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), DenDecision)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    marker = tmp_path / "socat-called"
    tools = tmp_path / "tools"
    tools.mkdir()
    fake_socat = tools / "socat"
    fake_socat.write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$SOCAT_MARKER"\n')
    fake_socat.chmod(0o700)
    env = {
        **os.environ,
        "PATH": f"{tools}:{os.environ['PATH']}",
        "SOCAT_MARKER": str(marker),
        "DEN_EGRESS_API_URL": f"http://127.0.0.1:{server.server_port}",
        "DEN_EGRESS_BEAR_SLUG": "test-bear",
        "DEN_EGRESS_RUN_ID": RUN_ID,
        "DEN_EGRESS_HOST": "docs.example.com",
        "DEN_EGRESS_IP": "1.1.1.1",
        "DEN_EGRESS_TOKEN": "bear_arm_test_only_token",
    }
    try:
        result = subprocess.run(["sh", str(RELAY), "connect"], env=env, capture_output=True, text=True, timeout=8)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
    assert len(DenDecision.requests) == 1
    path, authorization, request = DenDecision.requests[0]
    assert path == "/bearwire/v1/rpc"
    assert authorization == "Bearer bear_arm_test_only_token"
    assert request["method"] == "work.egress.check"
    assert request["params"] == {"bear_slug": "test-bear", "work_run_id": RUN_ID, "host": "docs.example.com"}
    if allowed:
        assert result.returncode == 0, result.stderr
        assert marker.read_text().strip() == "-T 30 STDIO TCP4:1.1.1.1:443"
    else:
        assert result.returncode != 0
        assert not marker.exists(), "denied checks cannot reach the external socket"


def test_relay_fails_closed_without_den_or_complete_run_context(tmp_path):
    marker = tmp_path / "socat-called"
    tools = tmp_path / "tools"
    tools.mkdir()
    fake_socat = tools / "socat"
    fake_socat.write_text('#!/bin/sh\ntouch "$SOCAT_MARKER"\n')
    fake_socat.chmod(0o700)
    env = {
        **os.environ,
        "PATH": f"{tools}:{os.environ['PATH']}",
        "SOCAT_MARKER": str(marker),
        "DEN_EGRESS_API_URL": "http://127.0.0.1:1",
        "DEN_EGRESS_BEAR_SLUG": "test-bear",
        "DEN_EGRESS_RUN_ID": RUN_ID,
        "DEN_EGRESS_HOST": "docs.example.com",
        "DEN_EGRESS_IP": "1.1.1.1",
        "DEN_EGRESS_TOKEN": "bear_arm_test_only_token",
    }
    unreachable = subprocess.run(["sh", str(RELAY), "connect"], env=env, capture_output=True, timeout=8)
    assert unreachable.returncode != 0
    assert not marker.exists()
    del env["DEN_EGRESS_TOKEN"]
    missing = subprocess.run(["sh", str(RELAY), "connect"], env=env, capture_output=True, timeout=8)
    assert missing.returncode != 0
    assert not marker.exists()
