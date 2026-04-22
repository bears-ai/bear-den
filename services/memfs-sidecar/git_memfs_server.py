#!/usr/bin/env python3
"""
Git smart-HTTP sidecar for self-hosted Letta. Letta proxies /v1/git/* to
LETTA_MEMFS_SERVICE_URL (e.g. http://bear-memfs:8285); this process implements
the /git/.../state.git path expected by the Letta server.

Upstream builds f"{LETTA_MEMFS_SERVICE_URL}/git/{path}" (httpx), so the base must be
a full http(s) URL. Mount the same path as Letta’s LocalStorageBackend: under
`~/.letta/memfs/repository/` (see MEMFS_BASE), sharing the `bear-letta` data volume.
"""
from __future__ import annotations

import os
import subprocess
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import urlparse

PORT = int(os.environ.get("PORT", "8285"))
_MEMFS = os.environ.get("MEMFS_BASE", "/root/.letta/memfs/repository")
MEMFS_BASE = Path(_MEMFS)
DEFAULT_ORG = os.environ.get("MEMFS_DEFAULT_ORG", "org-default")
BIND = os.environ.get("BIND", "0.0.0.0")


def find_or_create_repo(agent_id: str, org_id: str) -> Path:
    repo = MEMFS_BASE / org_id / agent_id / "repo.git"
    if not repo.exists():
        if MEMFS_BASE.exists():
            for org_dir in MEMFS_BASE.iterdir():
                if not org_dir.is_dir():
                    continue
                candidate = org_dir / agent_id / "repo.git"
                if candidate.exists():
                    return candidate
        repo.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "init", "--bare", str(repo)], check=True, capture_output=True
        )
        subprocess.run(
            ["git", "-C", str(repo), "config", "http.receivepack", "true"],
            check=True,
            capture_output=True,
        )
        print(f"[git-memfs] created bare repo {repo}", flush=True)
    return repo


class GitHTTPHandler(BaseHTTPRequestHandler):
    def _read_body(self) -> bytes:
        te = self.headers.get("Transfer-Encoding", "")
        if "chunked" in te.lower():
            body = b""
            while True:
                size_line = self.rfile.readline().strip()
                if not size_line:
                    break
                try:
                    chunk_size = int(size_line, 16)
                except ValueError:
                    break
                if chunk_size == 0:
                    self.rfile.readline()
                    break
                body += self.rfile.read(chunk_size)
                self.rfile.readline()
            return body
        n = int(self.headers.get("Content-Length", 0) or 0)
        return self.rfile.read(n) if n > 0 else b""

    def _parse_path(self) -> tuple[str | None, str | None, str]:
        """Return (agent_id, git_path_after_state_git, query)."""
        parsed = urlparse(self.path)
        parts = parsed.path.strip("/").split("/")
        if len(parts) < 3 or parts[0] != "git":
            return None, None, ""
        if parts[2] != "state.git":
            return None, None, ""
        agent_id = parts[1]
        git_op = "/" + "/".join(parts[3:]) if len(parts) > 3 else "/"
        return agent_id, git_op, parsed.query or ""

    def _run_backend(self) -> None:
        if self.path == "/health" or self.path.startswith("/health?"):
            self.send_response(200)
            self.end_headers()
            return

        agent_id, git_op, query = self._parse_path()
        if not agent_id or not git_op:
            self.send_error(400, "expected /git/{agent_id}/state.git/…")
            return

        org_id = self.headers.get("X-Organization-Id", DEFAULT_ORG) or DEFAULT_ORG

        repo_path = find_or_create_repo(agent_id, org_id)
        body = self._read_body()
        project_root = str(repo_path.parent).replace("\\", "/")
        path_info = f"/{repo_path.name}{git_op}"

        env: dict = {
            **os.environ,
            "GIT_HTTP_EXPORT_ALL": "1",
            "GIT_PROJECT_ROOT": project_root,
            "PATH_INFO": path_info,
            "QUERY_STRING": query,
            "REQUEST_METHOD": self.command,
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
            "CONTENT_LENGTH": str(len(body)),
            "HTTP_GIT_PROTOCOL": self.headers.get("Git-Protocol", ""),
            "REMOTE_ADDR": "127.0.0.1",
            "REMOTE_USER": "",
            "SERVER_NAME": "memfs",
            "SERVER_PORT": str(PORT),
            "SERVER_PROTOCOL": "HTTP/1.1",
        }
        result = subprocess.run(
            ["git", "http-backend"], input=body, capture_output=True, env=env
        )
        if result.returncode != 0:
            err = result.stderr.decode(errors="replace")
            print(f"[git-memfs] http-backend error: {err}", flush=True)
            self.send_error(500, "git http-backend failed")
            return

        raw = result.stdout
        sep = b"\r\n\r\n"
        pos = raw.find(sep)
        if pos == -1:
            sep = b"\n\n"
            pos = raw.find(sep)
        if pos == -1:
            self.send_error(502, "invalid http-backend output")
            return

        header_block = raw[:pos].decode(errors="replace")
        body_out = raw[pos + len(sep) :]
        status = 200
        headers: list[tuple[str, str]] = []
        for line in header_block.splitlines():
            if ":" in line:
                k, _, v = line.partition(":")
                k, v = k.strip(), v.strip()
                if k.lower() == "status":
                    try:
                        status = int(v.split()[0])
                    except (ValueError, IndexError):
                        pass
                else:
                    headers.append((k, v))

        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body_out)))
        self.end_headers()
        self.wfile.write(body_out)

    def do_GET(self) -> None:  # noqa: N802
        self._run_backend()

    def do_POST(self) -> None:  # noqa: N802
        self._run_backend()

    def log_message(self, fmt: str, *args) -> None:
        print(f"[git-memfs] {self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    MEMFS_BASE.mkdir(parents=True, exist_ok=True)
    print(f"[git-memfs] listen http://{BIND}:{PORT} base={MEMFS_BASE}", flush=True)
    HTTPServer((BIND, PORT), GitHTTPHandler).serve_forever()
