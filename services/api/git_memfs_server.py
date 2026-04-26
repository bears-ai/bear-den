#!/usr/bin/env python3
"""
Memory Manager: git smart-HTTP for self-hosted Letta. Letta proxies /v1/git/* to
LETTA_MEMFS_SERVICE_URL (e.g. http://bears-memfs-manager:8285); this process implements
the /git/.../state.git path expected by the Letta server.

Also exposes management endpoints for operator UIs (read-only; does not create repositories):
- GET /v1/management/agents/{agent_id}/head
- GET /v1/management/agents/{agent_id}/files

Upstream builds f"{LETTA_MEMFS_SERVICE_URL}/git/{path}" (httpx), so the base must be
a full http(s) URL. Mount the same path as Letta’s LocalStorageBackend: under
`~/.letta/memfs/repository/` (see MEMFS_BASE), sharing the `bears-letta` data volume.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import urlparse

PORT = int(os.environ.get("PORT", "8285"))
_MEMFS = os.environ.get("MEMFS_BASE", "/root/.letta/memfs/repository")
MEMFS_BASE = Path(_MEMFS)
DEFAULT_ORG = os.environ.get("MEMFS_DEFAULT_ORG", "org-default")
BIND = os.environ.get("BIND", "0.0.0.0")


def _commit_count_in_repo(repo: Path) -> int | None:
    r = subprocess.run(
        ["git", "-C", str(repo), "rev-list", "--all", "--count"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        return None
    try:
        return int((r.stdout or "0").strip() or 0)
    except ValueError:
        return None


def _is_usable_bare_repo(repo: Path) -> bool:
    if not repo.is_dir():
        return False
    c = _commit_count_in_repo(repo)
    return c is not None and c > 0


def _remove_bare_if_empty_or_broken(repo: Path) -> None:
    if not repo.exists():
        return
    c = _commit_count_in_repo(repo)
    if c is None or c == 0:
        print(
            f"[mem-manager] removing empty or invalid bare repo {repo} (commit_count={c})",
            flush=True,
        )
        shutil.rmtree(repo, ignore_errors=True)


def _create_bare_with_letta_initial_commit(repo: Path) -> None:
    """Match letta `GitOperations.create_repo`: at least one commit on `main` (clone/checkout safe)."""
    if repo.exists():
        shutil.rmtree(repo, ignore_errors=True)
    repo.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "w"
        work.mkdir()
        subprocess.run(
            ["git", "init", "-b", "main"],
            cwd=work,
            check=True,
            capture_output=True,
        )
        letta_dir = work / ".letta"
        letta_dir.mkdir()
        (letta_dir / "config.json").write_text('{"version": 1}', encoding="utf-8")
        subprocess.run(
            ["git", "add", ".letta/config.json"],
            cwd=work,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "-C", str(work), "config", "user.name", "Letta System"],
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "-C", str(work), "config", "user.email", "system@letta.ai"],
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "Initial commit"],
            cwd=work,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "clone", "--bare", str(work), str(repo)],
            check=True,
            capture_output=True,
        )
    subprocess.run(
        ["git", "-C", str(repo), "config", "http.receivepack", "true"],
        check=True,
        capture_output=True,
    )


def find_existing_repo_only(agent_id: str, org_id: str) -> Path | None:
    """Resolve a bare repo if it already exists and is usable. Never creates a repository."""
    repo = MEMFS_BASE / org_id / agent_id / "repo.git"
    if _is_usable_bare_repo(repo):
        return repo
    if MEMFS_BASE.exists():
        for org_dir in sorted(MEMFS_BASE.iterdir()):
            if not org_dir.is_dir():
                continue
            candidate = org_dir / agent_id / "repo.git"
            if not candidate.exists():
                continue
            if _is_usable_bare_repo(candidate):
                return candidate
    return None


def git_head_info(repo: Path) -> dict:
    """Latest commit on HEAD: sha, ISO date, message, short ref name if any."""
    r = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "log",
            "-1",
            "--format=%H\x1f%cI\x1f%s",
            "HEAD",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    out = (r.stdout or "").rstrip("\n")
    if "\x1f" not in out:
        msg = (r.stderr or r.stdout or "").strip()
        raise RuntimeError(f"git log: {msg}")
    parts = out.split("\x1f", 2)
    if len(parts) != 3:
        raise RuntimeError("git log: unexpected format")
    commit, date, message = parts
    ref_r = subprocess.run(
        ["git", "-C", str(repo), "symbolic-ref", "-q", "--short", "HEAD"],
        capture_output=True,
        text=True,
    )
    ref = (ref_r.stdout or "").strip() or "HEAD"
    return {
        "commit": commit,
        "date": date,
        "message": message,
        "ref": ref,
    }


def _git_last_commit_for_path(
    repo: Path, rel_path: str
) -> tuple[str | None, str | None]:
    r = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "log",
            "-1",
            "--format=%cI\x1f%s",
            "HEAD",
            "--",
            rel_path,
        ],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        return None, None
    out = (r.stdout or "").rstrip("\n")
    if not out or "\x1f" not in out:
        return None, None
    date, message = out.split("\x1f", 1)
    return (date or None), (message or None)


def _sort_tree_nodes(nodes: list[dict]) -> None:
    nodes.sort(
        key=lambda n: (
            0 if n.get("type") == "directory" else 1,
            str(n.get("name", "")).lower(),
        )
    )
    for n in nodes:
        children = n.get("children")
        if isinstance(children, list) and children:
            _sort_tree_nodes(children)


def git_repository_file_tree(repo: Path) -> list[dict]:
    r = subprocess.run(
        ["git", "-C", str(repo), "ls-tree", "-r", "--name-only", "HEAD"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        msg = (r.stderr or r.stdout or "").strip()
        raise RuntimeError(f"git ls-tree: {msg}")

    files = sorted(
        {line.strip() for line in (r.stdout or "").splitlines() if line.strip()}
    )
    if not files:
        return []

    dir_paths: set[str] = set()
    for f in files:
        parts = [p for p in f.split("/") if p]
        for i in range(1, len(parts)):
            dir_paths.add("/".join(parts[:i]))

    all_paths = set(files) | dir_paths
    ordered_paths = sorted(all_paths, key=lambda p: (p.count("/"), p.lower()))

    nodes: dict[str, dict] = {}
    for path in ordered_paths:
        is_dir = path in dir_paths
        lookup_path = f"{path}/" if is_dir else path
        last_date, last_message = _git_last_commit_for_path(repo, lookup_path)
        nodes[path] = {
            "name": path.rsplit("/", 1)[-1],
            "path": path,
            "type": "directory" if is_dir else "file",
            "last_commit_date": last_date,
            "last_commit_message": last_message,
            "children": [],
        }

    roots: list[dict] = []
    for path in ordered_paths:
        node = nodes[path]
        if "/" in path:
            parent_path = path.rsplit("/", 1)[0]
            parent = nodes.get(parent_path)
            if parent is None:
                roots.append(node)
            else:
                parent["children"].append(node)
        else:
            roots.append(node)

    _sort_tree_nodes(roots)
    return roots


def find_or_create_repo(agent_id: str, org_id: str) -> Path:
    """Resolve bare repo; align with local storage and Letta’s on-disk layout."""
    repo = MEMFS_BASE / org_id / agent_id / "repo.git"
    _remove_bare_if_empty_or_broken(repo)
    if _is_usable_bare_repo(repo):
        return repo
    if MEMFS_BASE.exists():
        for org_dir in sorted(MEMFS_BASE.iterdir()):
            if not org_dir.is_dir():
                continue
            candidate = org_dir / agent_id / "repo.git"
            if not candidate.exists():
                continue
            if _is_usable_bare_repo(candidate):
                return candidate
            _remove_bare_if_empty_or_broken(candidate)
    if not repo.parent.exists():
        repo.parent.mkdir(parents=True, exist_ok=True)
    elif not repo.parent.is_dir():
        raise OSError(f"not a directory: {repo.parent}")
    _create_bare_with_letta_initial_commit(repo)
    print(f"[mem-manager] created seeded bare repo {repo}", flush=True)
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

    def _try_management(self) -> bool:
        """Handle GET /v1/management/agents/{id}/head|files. Return True if handled."""
        if self.command != "GET":
            return False
        parsed = urlparse(self.path)
        parts = [p for p in parsed.path.strip("/").split("/") if p]
        if not (
            len(parts) == 5
            and parts[0] == "v1"
            and parts[1] == "management"
            and parts[2] == "agents"
        ):
            return False

        endpoint = parts[4]
        if endpoint not in {"head", "files"}:
            return False

        agent_id = parts[3]
        if not agent_id:
            return False

        org_id = self.headers.get("X-Organization-Id", DEFAULT_ORG) or DEFAULT_ORG
        repo = find_existing_repo_only(agent_id, org_id)
        if repo is None:
            self._send_json(404, {"error": "no_repository"})
            return True

        try:
            if endpoint == "head":
                info = git_head_info(repo)
                self._send_json(200, info)
                return True

            files = git_repository_file_tree(repo)
            self._send_json(200, {"files": files})
            return True
        except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
            self._send_json(500, {"error": str(e) or "git failed"})
            return True

    def _send_json(self, status: int, body: object) -> None:
        data = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

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
            "SERVER_NAME": "mem-manager",
            "SERVER_PORT": str(PORT),
            "SERVER_PROTOCOL": "HTTP/1.1",
        }
        result = subprocess.run(
            ["git", "http-backend"], input=body, capture_output=True, env=env
        )
        if result.returncode != 0:
            err = result.stderr.decode(errors="replace")
            print(f"[mem-manager] http-backend error: {err}", flush=True)
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
                lk = k.lower()
                if lk == "status":
                    try:
                        status = int(v.split()[0])
                    except (ValueError, IndexError):
                        pass
                elif lk == "content-length":
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
        if self._try_management():
            return
        self._run_backend()

    def do_POST(self) -> None:  # noqa: N802
        self._run_backend()

    def log_message(self, fmt: str, *args) -> None:
        print(f"[mem-manager] {self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    MEMFS_BASE.mkdir(parents=True, exist_ok=True)
    print(f"[mem-manager] listen http://{BIND}:{PORT} base={MEMFS_BASE}", flush=True)
    HTTPServer((BIND, PORT), GitHTTPHandler).serve_forever()
