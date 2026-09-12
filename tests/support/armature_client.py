import json
import os
import queue
import subprocess
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ARMATURE_BIN = Path(
    os.environ.get(
        "BEAR_ARMATURE_BIN", ROOT / "tools/bear-armature/target/debug/bear-armature"
    )
)
_ARMATURE_BUILT = False


class ArmatureClient:
    def __init__(self, proc):
        if proc.stdin is None or proc.stdout is None or proc.stderr is None:
            raise ValueError("Armature process must use piped stdin, stdout, and stderr")
        self.proc = proc
        self.responses = {}
        self.response_counts = {}
        self.notifications = []
        self.client_requests = queue.Queue()
        self.stderr_lines = []
        self._stdin = proc.stdin
        self._stdout = proc.stdout
        self._stderr = proc.stderr
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._err_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self._reader.start()
        self._err_reader.start()

    def _read_stdout(self):
        for line in self._stdout:
            if not line.strip():
                continue
            message = json.loads(line)
            if "id" in message and ("result" in message or "error" in message):
                request_id = str(message["id"])
                self.response_counts[request_id] = self.response_counts.get(request_id, 0) + 1
                self.responses[request_id] = message
            elif "method" in message:
                if message.get("method") == "session/update":
                    self.notifications.append(message)
                else:
                    self.client_requests.put(message)
            else:
                self.notifications.append(message)

    def _read_stderr(self):
        for line in self._stderr:
            self.stderr_lines.append(line.rstrip())

    def send(self, method, params=None, req_id=None):
        request_id = req_id or f"req-{int(time.time() * 1000)}"
        self._stdin.write(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": method,
                    "params": params or {},
                }
            )
            + "\n"
        )
        self._stdin.flush()
        return request_id

    def wait_response(self, req_id, timeout: float = 10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if str(req_id) in self.responses:
                return self.responses.pop(str(req_id))
            time.sleep(0.02)
        raise AssertionError(
            f"timed out waiting for response {req_id}; stderr={self.stderr_lines}"
        )

    def response_count(self, req_id):
        return self.response_counts.get(str(req_id), 0)

    def wait_any_client_request(self, timeout: float = 10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                return self.client_requests.get(timeout=0.05)
            except queue.Empty:
                continue
        raise AssertionError(
            f"timed out waiting for client request; stderr={self.stderr_lines}"
        )

    def wait_client_request(self, method, timeout: float = 10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            message = self.wait_any_client_request(
                timeout=max(0.05, deadline - time.time())
            )
            if message.get("method") == method:
                return message
            self.notifications.append(message)
        raise AssertionError(
            f"timed out waiting for client request {method}; stderr={self.stderr_lines}"
        )

    def respond(self, request, result=None, error=None):
        message = {"jsonrpc": "2.0", "id": request["id"]}
        if error is not None:
            message["error"] = error
        else:
            message["result"] = result or {}
        self._stdin.write(json.dumps(message) + "\n")
        self._stdin.flush()


def build_armature():
    global _ARMATURE_BUILT
    if _ARMATURE_BUILT:
        return
    subprocess.run(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(ROOT / "tools/bear-armature/Cargo.toml"),
        ],
        check=True,
        cwd=ROOT,
    )
    _ARMATURE_BUILT = True
