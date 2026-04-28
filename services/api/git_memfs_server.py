#!/usr/bin/env python3
"""
MemFS Manager: git smart-HTTP for self-hosted Letta. Letta proxies /v1/git/* to
LETTA_MEMFS_SERVICE_URL (e.g. http://bears-memfs-manager:8285); this process implements
the /git/.../state.git path expected by the Letta server.

Also exposes management endpoints for operator UIs (read-only; does not create repositories):
- GET /v1/management/agents/{agent_id}/head
- GET /v1/management/agents/{agent_id}/files
- GET /v1/management/agents/{agent_id}/status
- GET /v1/management/diagnostics
- GET /v1/management/activity?agent_id={agent_id}&limit=100

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
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

PORT = int(os.environ.get("PORT", "8285"))
_MEMFS = os.environ.get("MEMFS_BASE", "/root/.letta/memfs/repository")
MEMFS_BASE = Path(_MEMFS)
LETTA_DEFAULT_ORG_ID = "org-00000000-0000-4000-8000-000000000000"
DEFAULT_ORG = os.environ.get("MEMFS_DEFAULT_ORG", LETTA_DEFAULT_ORG_ID)
BIND = os.environ.get("BIND", "0.0.0.0")
ACTIVITY_LOG = Path(
    os.environ.get("MEMFS_ACTIVITY_LOG", str(MEMFS_BASE.parent / "activity.jsonl"))
)
ACTIVITY_LOG_MAX_BYTES = int(os.environ.get("MEMFS_ACTIVITY_LOG_MAX_BYTES", "1048576"))


def log_activity(event: str, **fields: object) -> None:
    entry = {
        "ts": time.time(),
        "event": event,
        **fields,
    }
    line = json.dumps(entry, sort_keys=True)
    print(f"[memfs-manager] activity {line}", flush=True)
    try:
        ACTIVITY_LOG.parent.mkdir(parents=True, exist_ok=True)
        if (
            ACTIVITY_LOG.exists()
            and ACTIVITY_LOG.stat().st_size > ACTIVITY_LOG_MAX_BYTES
        ):
            ACTIVITY_LOG.replace(ACTIVITY_LOG.with_suffix(".jsonl.1"))
        with ACTIVITY_LOG.open("a", encoding="utf-8") as f:
            f.write(line + "\n")
    except OSError as e:
        print(f"[memfs-manager] activity log write failed: {e}", flush=True)


def recent_activity(agent_id: str | None = None, limit: int = 100) -> list[dict]:
    if limit < 1:
        limit = 1
    if limit > 500:
        limit = 500
    if not ACTIVITY_LOG.exists():
        return []
    try:
        lines = ACTIVITY_LOG.read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    out: list[dict] = []
    for line in reversed(lines):
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if agent_id and entry.get("agent_id") != agent_id:
            continue
        out.append(entry)
        if len(out) >= limit:
            break
    out.reverse()
    return out


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
        log_activity("repo_removed_empty_or_broken", repo=str(repo), commit_count=c)
        shutil.rmtree(repo, ignore_errors=True)


def _create_bare_with_letta_initial_commit(repo: Path) -> None:
    """Match letta `GitOperations.create_repo`: at least one commit on `main` (clone/checkout safe)."""
    if repo.exists():
        log_activity("repo_recreate_remove_existing", repo=str(repo))
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
    commit_count = _commit_count_in_repo(repo)
    log_activity("repo_created_seeded", repo=str(repo), commit_count=commit_count)


def discover_single_org_id() -> str | None:
    """Return the only existing org directory when the memfs layout is unambiguous."""
    if not MEMFS_BASE.exists():
        return None
    orgs = sorted(p.name for p in MEMFS_BASE.iterdir() if p.is_dir())
    return orgs[0] if len(orgs) == 1 else None


def resolve_org_id(header_org_id: str | None = None) -> str:
    """Resolve org id for git clients that do not send X-Organization-Id."""
    explicit = (header_org_id or "").strip()
    if explicit:
        return explicit
    discovered = discover_single_org_id()
    if discovered:
        return discovered
    return DEFAULT_ORG


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

    # Check total commit count
    commit_count = _commit_count_in_repo(repo)

    return {
        "commit": commit,
        "date": date,
        "message": message,
        "ref": ref,
        "commit_count": commit_count,
        "warning": "Repository only has initial commit - memory may not be synced"
        if commit_count is not None and commit_count <= 1
        else None,
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


def git_repository_file_paths(repo: Path) -> list[str]:
    r = subprocess.run(
        ["git", "-C", str(repo), "ls-tree", "-r", "--name-only", "HEAD"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        msg = (r.stderr or r.stdout or "").strip()
        raise RuntimeError(f"git ls-tree: {msg}")
    return sorted(
        {line.strip() for line in (r.stdout or "").splitlines() if line.strip()}
    )


def repository_status(agent_id: str, org_id: str) -> dict:
    repo = find_existing_repo_only(agent_id, org_id)
    if repo is None:
        return {
            "agent_id": agent_id,
            "org_id": org_id,
            "state": "missing_repo",
            "repo_exists": False,
            "repo_usable": False,
            "warning": "No usable memfs repository exists for this agent.",
        }

    commit_count = _commit_count_in_repo(repo)
    files = git_repository_file_paths(repo)
    memory_files = [p for p in files if p != ".letta/config.json"]
    state = "ok"
    warning = None
    if commit_count is None:
        state = "git_error"
        warning = "Git could not count commits for this repository."
    elif commit_count <= 1 and not memory_files:
        state = "seed_only"
        warning = "Repository only contains the seeded .letta/config.json commit; agent memory has not synced to memfs."
    elif not memory_files:
        state = "no_memory_files"
        warning = (
            "Repository has commits but no memory files other than .letta/config.json."
        )

    return {
        "agent_id": agent_id,
        "org_id": org_id,
        "state": state,
        "warning": warning,
        "repo": str(repo),
        "repo_exists": True,
        "repo_usable": True,
        "head": git_head_info(repo),
        "commit_count": commit_count,
        "file_count": len(files),
        "memory_file_count": len(memory_files),
        "files": files[:50],
    }


def find_or_create_repo(agent_id: str, org_id: str) -> Path:
    """Resolve bare repo; align with local storage and Letta's on-disk layout."""
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
        """Handle GET /v1/management/agents/{id}/head|files|status."""
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
        if endpoint not in {"head", "files", "status"}:
            return False

        agent_id = parts[3]
        if not agent_id:
            return False

        org_id = resolve_org_id(self.headers.get("X-Organization-Id"))
        if endpoint == "status":
            try:
                status = repository_status(agent_id, org_id)
                status["recent_activity"] = recent_activity(agent_id, 50)
                code = 200 if status.get("state") != "missing_repo" else 404
                log_activity(
                    "management_status",
                    agent_id=agent_id,
                    org_id=org_id,
                    status=code,
                    state=status.get("state"),
                )
                self._send_json(code, status)
                return True
            except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
                log_activity(
                    "management_status_error",
                    agent_id=agent_id,
                    org_id=org_id,
                    error=str(e),
                )
                self._send_json(500, {"error": str(e) or "git failed"})
                return True

        repo = find_existing_repo_only(agent_id, org_id)
        if repo is None:
            log_activity(
                "management_no_repository",
                agent_id=agent_id,
                org_id=org_id,
                endpoint=endpoint,
            )
            self._send_json(404, {"error": "no_repository"})
            return True

        try:
            if endpoint == "head":
                info = git_head_info(repo)
                log_activity(
                    "management_head",
                    agent_id=agent_id,
                    org_id=org_id,
                    repo=str(repo),
                    commit_count=info.get("commit_count"),
                )
                self._send_json(200, info)
                return True

            files = git_repository_file_tree(repo)
            log_activity(
                "management_files",
                agent_id=agent_id,
                org_id=org_id,
                repo=str(repo),
                file_count=len(git_repository_file_paths(repo)),
            )
            self._send_json(200, {"files": files})
            return True
        except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
            log_activity(
                "management_error",
                agent_id=agent_id,
                org_id=org_id,
                endpoint=endpoint,
                error=str(e),
            )
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
            self.send_error(
                400,
                "expected /git/{agent_id}/state.git/...".encode(
                    "latin-1", errors="replace"
                ).decode("latin-1"),
            )
            return

        org_id = resolve_org_id(self.headers.get("X-Organization-Id"))

        repo_path = find_or_create_repo(agent_id, org_id)
        body = self._read_body()
        log_activity(
            "git_request",
            agent_id=agent_id,
            org_id=org_id,
            method=self.command,
            git_op=git_op,
            query=query,
            content_length=len(body),
            repo=str(repo_path),
        )
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
            "SERVER_NAME": "memfs-manager",
            "SERVER_PORT": str(PORT),
            "SERVER_PROTOCOL": "HTTP/1.1",
        }
        result = subprocess.run(
            ["git", "http-backend"], input=body, capture_output=True, env=env
        )
        if result.returncode != 0:
            err = result.stderr.decode(errors="replace")
            log_activity(
                "git_backend_error",
                agent_id=agent_id,
                org_id=org_id,
                git_op=git_op,
                error=err,
            )
            self.send_error(500, "git http-backend failed")
            return

        raw = result.stdout
        sep = b"\r\n\r\n"
        pos = raw.find(sep)
        if pos == -1:
            sep = b"\n\n"
            pos = raw.find(sep)
        if pos == -1:
            log_activity(
                "git_backend_invalid_output",
                agent_id=agent_id,
                org_id=org_id,
                git_op=git_op,
            )
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

        log_activity(
            "git_response",
            agent_id=agent_id,
            org_id=org_id,
            method=self.command,
            git_op=git_op,
            status=status,
            response_bytes=len(body_out),
            commit_count=_commit_count_in_repo(repo_path),
        )

        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body_out)))
        self.end_headers()
        self.wfile.write(body_out)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path == "/v1/management/diagnostics":
            self._send_diagnostics()
            return
        if parsed.path == "/v1/management/activity":
            self._send_activity(parsed.query)
            return
        if self._try_management():
            return
        self._run_backend()

    def _send_activity(self, query: str) -> None:
        params = parse_qs(query)
        agent_id = (params.get("agent_id") or [None])[0]
        try:
            limit = int((params.get("limit") or ["100"])[0])
        except ValueError:
            limit = 100
        self._send_json(
            200,
            {
                "activity_log": str(ACTIVITY_LOG),
                "agent_id": agent_id,
                "events": recent_activity(agent_id, limit),
            },
        )

    def _send_diagnostics(self) -> None:
        """Return diagnostics about the memfs state."""
        result = {
            "memfs_base": str(MEMFS_BASE),
            "memfs_base_exists": MEMFS_BASE.exists(),
            "default_org": DEFAULT_ORG,
            "discovered_single_org": discover_single_org_id(),
            "letta_default_org_id": LETTA_DEFAULT_ORG_ID,
            "sync_note": "This service verifies git repository state only; Letta's Postgres block cache must still be checked through the Letta API.",
            "activity_log": str(ACTIVITY_LOG),
            "activity_log_exists": ACTIVITY_LOG.exists(),
            "timestamp": time.time(),
            "repos": [],
            "recent_activity": recent_activity(limit=50),
        }
        if MEMFS_BASE.exists():
            for org_dir in sorted(MEMFS_BASE.iterdir()):
                if not org_dir.is_dir():
                    continue
                org_info = {
                    "org": org_dir.name,
                    "agents": [],
                }
                for agent_dir in sorted(org_dir.iterdir()):
                    if not agent_dir.is_dir():
                        continue
                    repo = agent_dir / "repo.git"
                    info = {
                        "agent_id": agent_dir.name,
                        "repo_exists": repo.exists(),
                        "repo_usable": _is_usable_bare_repo(repo)
                        if repo.exists()
                        else False,
                    }
                    if repo.exists() and _is_usable_bare_repo(repo):
                        commit_count = _commit_count_in_repo(repo)
                        info["commit_count"] = commit_count
                        info["has_only_initial_commit"] = (
                            commit_count == 1 if commit_count else False
                        )
                        try:
                            files = git_repository_file_paths(repo)
                            info["file_count"] = len(files)
                            info["memory_file_count"] = len(
                                [p for p in files if p != ".letta/config.json"]
                            )
                            info["files"] = files[:10]
                        except Exception as e:
                            info["files_error"] = str(e)
                    org_info["agents"].append(info)
                result["repos"].append(org_info)
        self._send_json(200, result)

    def do_POST(self) -> None:  # noqa: N802
        self._run_backend()

    def log_message(self, fmt: str, *args) -> None:
        print(f"[memfs-manager] {self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    MEMFS_BASE.mkdir(parents=True, exist_ok=True)
    print(f"[memfs-manager] listen http://{BIND}:{PORT} base={MEMFS_BASE}", flush=True)
    HTTPServer((BIND, PORT), GitHTTPHandler).serve_forever()
