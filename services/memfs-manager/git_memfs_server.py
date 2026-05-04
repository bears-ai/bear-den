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
LOG_HEALTHCHECKS = os.environ.get("MEMFS_LOG_HEALTHCHECKS", "").lower() in {
    "1",
    "true",
    "yes",
    "on",
}
ACTIVITY_LOG = Path(
    os.environ.get("MEMFS_ACTIVITY_LOG", str(MEMFS_BASE.parent / "activity.jsonl"))
)
ACTIVITY_LOG_MAX_BYTES = int(os.environ.get("MEMFS_ACTIVITY_LOG_MAX_BYTES", "1048576"))
BEARS_CANONICAL_BASE = Path(
    os.environ.get("BEARS_CANONICAL_MEMFS_BASE", str(MEMFS_BASE.parent / "bears"))
)
VIEW_REGISTRY_PATH = Path(
    os.environ.get("MEMFS_VIEW_REGISTRY", str(MEMFS_BASE.parent / "bear_views.json"))
)
ROLE_BRANCHES = {"talk", "pair", "curate", "work", "watch"}
ROLE_ALLOWED_PREFIXES = {
    "talk": ["talk/"],
    "pair": ["pair/"],
    "curate": ["curate/", "core/"],
    "work": ["work/"],
    "watch": ["watch/"],
}


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


def _git(
    *args: str, cwd: Path | None = None, check: bool = True
) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=check
    )


def _branch_tip(repo: Path, branch: str) -> str | None:
    r = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "--verify", f"refs/heads/{branch}"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        return None
    return (r.stdout or "").strip() or None


def _is_ancestor(repo: Path, maybe_ancestor: str, descendant: str) -> bool:
    r = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "merge-base",
            "--is-ancestor",
            maybe_ancestor,
            descendant,
        ],
        capture_output=True,
        text=True,
    )
    return r.returncode == 0


def _changed_paths(repo: Path, old: str | None, new: str) -> list[str]:
    if old:
        r = _git("-C", str(repo), "diff", "--name-only", old, new)
    else:
        r = _git(
            "-C",
            str(repo),
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            new,
        )
    return [line.strip() for line in (r.stdout or "").splitlines() if line.strip()]


def _paths_allowed(role: str, paths: list[str]) -> tuple[bool, str | None]:
    prefixes = ROLE_ALLOWED_PREFIXES.get(role, [])
    for path in paths:
        if not any(path.startswith(prefix) for prefix in prefixes):
            return False, path
    return True, None


def _read_view_registry() -> dict:
    if not VIEW_REGISTRY_PATH.exists():
        return {"version": 1, "views": {}}
    try:
        data = json.loads(VIEW_REGISTRY_PATH.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            return {"version": 1, "views": {}}
        data.setdefault("version", 1)
        data.setdefault("views", {})
        return data
    except (OSError, json.JSONDecodeError):
        return {"version": 1, "views": {}}


def _write_view_registry(data: dict) -> None:
    VIEW_REGISTRY_PATH.parent.mkdir(parents=True, exist_ok=True)
    tmp = VIEW_REGISTRY_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
    tmp.replace(VIEW_REGISTRY_PATH)


def _view_record(agent_id: str) -> dict | None:
    return _read_view_registry().get("views", {}).get(agent_id)


def _canonical_repo_path(bear_id: str) -> Path:
    return BEARS_CANONICAL_BASE / f"{bear_id}.git"


def _view_repo_path(org_id: str, agent_id: str) -> Path:
    return MEMFS_BASE / org_id / agent_id / "repo.git"


def _repo_is_quarantined(repo: Path) -> bool:
    return (repo / "BEARS_QUARANTINED").exists()


def _set_quarantine(repo: Path, reason: str) -> None:
    repo.mkdir(parents=True, exist_ok=True)
    (repo / "BEARS_QUARANTINED").write_text(reason, encoding="utf-8")


def _clear_quarantine(repo: Path) -> None:
    marker = repo / "BEARS_QUARANTINED"
    if marker.exists():
        marker.unlink()


def _canonical_hook_contents() -> str:
    return r"""#!/usr/bin/env sh
set -eu
zero="0000000000000000000000000000000000000000"
allowed_for_ref() {
  case "$1" in
    refs/heads/talk) printf '%s\n' "talk/" ;;
    refs/heads/pair) printf '%s\n' "pair/" ;;
    refs/heads/curate) printf '%s\n' "curate/" "core/" ;;
    refs/heads/work) printf '%s\n' "work/" ;;
    refs/heads/watch) printf '%s\n' "watch/" ;;
    *) printf '%s\n' "" ;;
  esac
}
while read old new ref; do
  case "$ref" in refs/heads/talk|refs/heads/pair|refs/heads/curate|refs/heads/work|refs/heads/watch) ;; *) continue ;; esac
  [ "$new" = "$zero" ] && continue
  branch="${ref#refs/heads/}"
  allowed="$(allowed_for_ref "$ref")"
  if [ "$old" = "$zero" ]; then
    paths="$(git diff-tree --root --no-commit-id --name-only -r "$new")"
  else
    paths="$(git diff --name-only "$old" "$new")"
  fi
  printf '%s\n' "$paths" | while IFS= read -r path; do
    [ -z "$path" ] && continue
    case "$ref:$path" in
      refs/heads/talk:talk/*|refs/heads/pair:pair/*|refs/heads/curate:curate/*|refs/heads/curate:core/*|refs/heads/work:work/*|refs/heads/watch:watch/*) ;;
      *) echo "branch '$branch' attempted to write to '$path'; allowed path prefixes are:" >&2; printf '%s\n' "$allowed" | sed 's/^/  - /' >&2; exit 1 ;;
    esac
  done
done
"""


def _write_canonical_hook(repo: Path) -> None:
    hooks = repo / "hooks"
    hooks.mkdir(parents=True, exist_ok=True)
    hook = hooks / "pre-receive"
    hook.write_text(_canonical_hook_contents(), encoding="utf-8")
    hook.chmod(0o755)


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


def _ensure_canonical_branch(repo: Path, bear_id: str, role: str) -> None:
    if _branch_tip(repo, role):
        return
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "w"
        work.mkdir()
        _git("init", "-b", role, cwd=work)
        _git("-C", str(work), "config", "user.name", "BEARS MemFS Sidecar")
        _git("-C", str(work), "config", "user.email", "memfs-sidecar@bears.local")
        paths = {
            "talk": ["talk/tasks"],
            "pair": ["pair/tasks"],
            "curate": ["curate", "core/tasks", "core/results"],
            "work": ["work/results"],
            "watch": ["watch/observations", "watch/subscriptions"],
        }[role]
        for rel in paths:
            d = work / rel
            d.mkdir(parents=True, exist_ok=True)
            (d / ".gitkeep").write_text("", encoding="utf-8")
        _git("add", ".", cwd=work)
        _git("commit", "-m", f"Initialize {role} branch for bear {bear_id}", cwd=work)
        _git("remote", "add", "origin", str(repo), cwd=work)
        _git("push", "origin", f"HEAD:refs/heads/{role}", cwd=work)


def ensure_canonical_repo(bear_id: str) -> Path:
    repo = _canonical_repo_path(bear_id)
    if not repo.exists():
        repo.parent.mkdir(parents=True, exist_ok=True)
        _git("init", "--bare", str(repo))
        _git("--git-dir", str(repo), "config", "http.receivepack", "true")
    _write_canonical_hook(repo)
    for role in sorted(ROLE_BRANCHES):
        _ensure_canonical_branch(repo, bear_id, role)
    return repo


def _reset_view_to_canonical(view_repo: Path, canonical_repo: Path, role: str) -> None:
    canonical_tip = _branch_tip(canonical_repo, role)
    if not canonical_tip:
        raise RuntimeError(f"canonical branch missing: {role}")
    if not view_repo.exists():
        view_repo.parent.mkdir(parents=True, exist_ok=True)
        _git("clone", "--shared", "--bare", str(canonical_repo), str(view_repo))
    _git(
        "--git-dir",
        str(view_repo),
        "fetch",
        str(canonical_repo),
        f"refs/heads/{role}:refs/heads/main",
    )
    _git("--git-dir", str(view_repo), "symbolic-ref", "HEAD", "refs/heads/main")
    _git("--git-dir", str(view_repo), "config", "http.receivepack", "true")
    _clear_quarantine(view_repo)


def _forward_view_to_canonical(
    record: dict, old_tip: str | None = None
) -> tuple[str, str | None]:
    role = str(record["role"])
    bear_id = str(record["bear_id"])
    agent_id = str(record["agent_id"])
    canonical = Path(record["canonical_repo"])
    view = Path(record["view_repo"])
    if _repo_is_quarantined(view):
        return "quarantined", "view is quarantined; operator action required"
    view_tip = _branch_tip(view, "main")
    canonical_tip = _branch_tip(canonical, role)
    if not view_tip:
        return "missing_view_tip", "view main branch missing"
    if canonical_tip == view_tip:
        return "already_current", None
    if canonical_tip and _is_ancestor(view, view_tip, canonical_tip):
        _reset_view_to_canonical(view, canonical, role)
        return "fast_forwarded_view", None
    if canonical_tip and not _is_ancestor(view, canonical_tip, view_tip):
        reason = f"view and canonical diverged for role {role}: canonical={canonical_tip}, view={view_tip}"
        _set_quarantine(view, reason)
        log_activity(
            "view_quarantined",
            bear_id=bear_id,
            role=role,
            agent_id=agent_id,
            reason=reason,
        )
        return "quarantined", reason
    paths = _changed_paths(view, canonical_tip, view_tip)
    ok, bad_path = _paths_allowed(role, paths)
    if not ok:
        reason = f"canonical would reject role {role} path {bad_path}"
        _set_quarantine(view, reason)
        log_activity(
            "view_quarantined",
            bear_id=bear_id,
            role=role,
            agent_id=agent_id,
            reason=reason,
            bad_path=bad_path,
        )
        return "quarantined", reason
    try:
        _git(
            "--git-dir",
            str(canonical),
            "fetch",
            str(view),
            f"refs/heads/main:refs/heads/{role}",
        )
    except subprocess.CalledProcessError as e:
        reason = (e.stderr or e.stdout or str(e)).strip()
        _set_quarantine(view, reason)
        log_activity(
            "forward_failed_quarantined",
            bear_id=bear_id,
            role=role,
            agent_id=agent_id,
            reason=reason,
        )
        return "quarantined", reason
    log_activity(
        "forward_ok",
        bear_id=bear_id,
        role=role,
        agent_id=agent_id,
        old_tip=old_tip,
        new_tip=view_tip,
        canonical_tip=canonical_tip,
    )
    return "forwarded", None


def ensure_view_repo(agent_id: str, org_id: str, record: dict) -> Path:
    canonical = ensure_canonical_repo(str(record["bear_id"]))
    view = _view_repo_path(org_id, agent_id)
    record["canonical_repo"] = str(canonical)
    record["view_repo"] = str(view)
    _reset_view_to_canonical(view, canonical, str(record["role"]))
    return view


def find_or_create_repo(agent_id: str, org_id: str) -> Path:
    """Resolve bare repo; align with local storage and Letta's on-disk layout."""
    record = _view_record(agent_id)
    if record:
        return ensure_view_repo(agent_id, org_id, record)
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

    def _try_view_management_get(self) -> bool:
        if self.command != "GET":
            return False
        parsed = urlparse(self.path)
        parts = [p for p in parsed.path.strip("/").split("/") if p]
        if parts == ["v1", "management", "bears"]:
            data = _read_view_registry()
            self._send_json(200, data)
            return True
        if (
            len(parts) == 6
            and parts[:3] == ["v1", "management", "bears"]
            and parts[4] == "roles"
        ):
            bear_id = parts[3]
            role = parts[5]
            data = _read_view_registry()
            rows = []
            for record in data.get("views", {}).values():
                if (
                    str(record.get("bear_id")) == bear_id
                    and str(record.get("role")) == role
                ):
                    rows.append(self._view_health(record))
            self._send_json(200, {"bear_id": bear_id, "role": role, "views": rows})
            return True
        return False

    def _view_health(self, record: dict) -> dict:
        role = str(record.get("role", ""))
        canonical = Path(
            str(
                record.get(
                    "canonical_repo",
                    _canonical_repo_path(str(record.get("bear_id", ""))),
                )
            )
        )
        view = Path(str(record.get("view_repo", "")))
        canonical_tip = _branch_tip(canonical, role) if canonical.exists() else None
        view_tip = _branch_tip(view, "main") if view.exists() else None
        quarantined = _repo_is_quarantined(view) if view.exists() else False
        reason = None
        marker = view / "BEARS_QUARANTINED"
        if marker.exists():
            reason = marker.read_text(encoding="utf-8", errors="replace")
        state = "ok"
        if quarantined:
            state = "quarantined"
        elif not canonical.exists():
            state = "missing_canonical"
        elif not view.exists():
            state = "missing_view"
        elif canonical_tip != view_tip:
            state = "drift"
        return {
            **record,
            "state": state,
            "canonical_exists": canonical.exists(),
            "view_exists": view.exists(),
            "canonical_tip": canonical_tip,
            "view_tip": view_tip,
            "quarantined": quarantined,
            "diagnostic": reason,
        }

    def _try_view_management_post(self) -> bool:
        if self.command != "POST":
            return False
        parsed = urlparse(self.path)
        parts = [p for p in parsed.path.strip("/").split("/") if p]
        if parts == ["v1", "management", "views", "register"]:
            try:
                body = json.loads(self._read_body().decode("utf-8") or "{}")
                agent_id = str(body["agent_id"]).strip()
                bear_id = str(body["bear_id"]).strip()
                role = str(body["role"]).strip()
                org_id = str(
                    body.get("org_id")
                    or resolve_org_id(self.headers.get("X-Organization-Id"))
                ).strip()
                if not agent_id or not bear_id or role not in ROLE_BRANCHES:
                    raise ValueError("agent_id, bear_id, and valid role are required")
                canonical = ensure_canonical_repo(bear_id)
                record = {
                    "agent_id": agent_id,
                    "bear_id": bear_id,
                    "role": role,
                    "org_id": org_id,
                    "canonical_repo": str(canonical),
                    "canonical_branch": role,
                    "view_repo": str(_view_repo_path(org_id, agent_id)),
                    "registered_at": time.time(),
                }
                ensure_view_repo(agent_id, org_id, record)
                data = _read_view_registry()
                data.setdefault("views", {})[agent_id] = record
                _write_view_registry(data)
                log_activity(
                    "view_registered",
                    agent_id=agent_id,
                    bear_id=bear_id,
                    role=role,
                    org_id=org_id,
                )
                self._send_json(200, {"ok": True, "view": self._view_health(record)})
                return True
            except Exception as e:
                self._send_json(400, {"ok": False, "error": str(e)})
                return True
        if (
            len(parts) == 5
            and parts[:3] == ["v1", "management", "views"]
            and parts[4] == "reconcile"
        ):
            agent_id = parts[3]
            record = _view_record(agent_id)
            if not record:
                self._send_json(404, {"ok": False, "error": "view_not_registered"})
                return True
            try:
                ensure_view_repo(
                    agent_id, str(record.get("org_id") or resolve_org_id(None)), record
                )
                status, reason = _forward_view_to_canonical(record)
                data = _read_view_registry()
                data.setdefault("views", {})[agent_id] = record
                _write_view_registry(data)
                self._send_json(
                    200,
                    {
                        "ok": status not in {"quarantined", "missing_view_tip"},
                        "status": status,
                        "reason": reason,
                        "view": self._view_health(record),
                    },
                )
                return True
            except Exception as e:
                self._send_json(
                    500,
                    {"ok": False, "error": str(e), "view": self._view_health(record)},
                )
                return True
        return False

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

        record = _view_record(agent_id)
        if (
            record
            and self.command == "POST"
            and "git-receive-pack" in git_op
            and 200 <= status < 300
        ):
            old_tip = record.get("last_view_tip")
            record["canonical_repo"] = str(_canonical_repo_path(str(record["bear_id"])))
            record["view_repo"] = str(repo_path)
            forward_status, forward_reason = _forward_view_to_canonical(
                record, old_tip=old_tip
            )
            record["last_view_tip"] = _branch_tip(repo_path, "main")
            record["last_forward_status"] = forward_status
            record["last_forward_reason"] = forward_reason
            record["last_forward_at"] = time.time()
            data = _read_view_registry()
            data.setdefault("views", {})[agent_id] = record
            _write_view_registry(data)
            log_activity(
                "view_forward_result",
                agent_id=agent_id,
                org_id=org_id,
                bear_id=record.get("bear_id"),
                role=record.get("role"),
                status=forward_status,
                reason=forward_reason,
            )

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
        if self._try_view_management_get():
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
        if self._try_view_management_post():
            return
        self._run_backend()

    def log_message(self, fmt: str, *args) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/health" and not LOG_HEALTHCHECKS:
            return
        print(f"[memfs-manager] {self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    MEMFS_BASE.mkdir(parents=True, exist_ok=True)
    print(f"[memfs-manager] listen http://{BIND}:{PORT} base={MEMFS_BASE}", flush=True)
    HTTPServer((BIND, PORT), GitHTTPHandler).serve_forever()
