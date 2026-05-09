#!/usr/bin/env python3
# pyright: reportAny=false, reportExplicitAny=false, reportUnknownVariableType=false, reportUnknownMemberType=false, reportUnknownArgumentType=false, reportUnknownParameterType=false, reportUnknownLambdaType=false, reportUnusedCallResult=false, reportUnusedFunction=false, reportImplicitOverride=false
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
import re
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any, TypeAlias, cast
from urllib.parse import parse_qs, unquote, urlparse

JSONValue: TypeAlias = Any
JSONDict: TypeAlias = dict[str, Any]
ResponseDict: TypeAlias = dict[str, Any]
TreeNode: TypeAlias = dict[str, Any]
ViewRecord: TypeAlias = dict[str, Any]
ViewRegistry: TypeAlias = dict[str, Any]
ReconcileResult: TypeAlias = dict[str, Any]
ReconcileSummary: TypeAlias = dict[str, Any]

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
RECONCILE_INTERVAL_SECONDS = int(
    os.environ.get("MEMFS_RECONCILE_INTERVAL_SECONDS", "60")
)
BEARS_CANONICAL_BASE = Path(
    os.environ.get("BEARS_CANONICAL_MEMFS_BASE", str(MEMFS_BASE.parent / "bears"))
)
VIEW_REGISTRY_PATH = Path(
    os.environ.get("MEMFS_VIEW_REGISTRY", str(MEMFS_BASE.parent / "bear_views.json"))
)
ROLE_BRANCHES = {"talk", "pair", "curate", "work", "watch"}
ROLE_READ_ALLOWED_PREFIXES = {
    "talk": ["talk/", "core/"],
    "pair": ["pair/", "core/"],
    "curate": ["talk/", "pair/", "curate/", "work/", "watch/", "core/"],
    "work": ["work/", "core/"],
    "watch": ["watch/", "core/"],
}
ROLE_WRITE_ALLOWED_PREFIXES = {
    "talk": ["talk/"],
    "pair": ["pair/"],
    "curate": ["curate/", "core/"],
    "work": ["work/"],
    "watch": ["watch/"],
}
MEMORY_ENTRY_KIND_DIRS = {
    "note": "notes",
    "log": "logs",
    "decision": "decisions",
    "reflection": "reflections",
    "scratch": "scratch",
    "summary": "summaries",
}
MEMORY_TREE_MAX_FILES = int(os.environ.get("MEMFS_MEMORY_TREE_MAX_FILES", "500"))
MEMORY_SEARCH_MAX_RESULTS = int(os.environ.get("MEMFS_MEMORY_SEARCH_MAX_RESULTS", "50"))
MEMORY_FILE_MAX_BYTES = int(os.environ.get("MEMFS_MEMORY_FILE_MAX_BYTES", "131072"))
MEMORY_SEARCH_FILE_MAX_BYTES = int(
    os.environ.get("MEMFS_MEMORY_SEARCH_FILE_MAX_BYTES", "65536")
)


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


def recent_activity(agent_id: str | None = None, limit: int = 100) -> list[JSONDict]:
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
    out: list[JSONDict] = []
    for line in reversed(lines):
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(entry, dict):
            continue
        typed_entry = cast(JSONDict, entry)
        if agent_id and typed_entry.get("agent_id") != agent_id:
            continue
        out.append(typed_entry)
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
) -> subprocess.CompletedProcess[str]:
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


def _merge_base(repo: Path, a: str | None, b: str | None) -> str | None:
    if not a or not b:
        return None
    r = subprocess.run(
        ["git", "-C", str(repo), "merge-base", a, b],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        return None
    return (r.stdout or "").strip() or None


def _rev_count(repo: Path, rev_range: str) -> int | None:
    r = subprocess.run(
        ["git", "-C", str(repo), "rev-list", "--count", rev_range],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        return None
    try:
        return int((r.stdout or "0").strip() or 0)
    except ValueError:
        return None


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


def _write_paths_allowed(role: str, paths: list[str]) -> tuple[bool, str | None]:
    prefixes = ROLE_WRITE_ALLOWED_PREFIXES.get(role, [])
    for path in paths:
        if not any(path.startswith(prefix) for prefix in prefixes):
            return False, path
    return True, None


def _normalize_memory_path(raw_path: str) -> str:
    path = unquote(raw_path).strip().lstrip("/")
    if not path or path.endswith("/"):
        raise ValueError("path is required")
    parts = [part for part in path.split("/") if part]
    if any(part in {".", ".."} for part in parts):
        raise ValueError("path must not contain . or .. components")
    return "/".join(parts)


def _role_memory_prefixes(role: str) -> list[str]:
    return ROLE_READ_ALLOWED_PREFIXES.get(role, [])


def _role_memory_path_allowed(role: str, path: str) -> bool:
    prefixes = _role_memory_prefixes(role)
    return bool(prefixes) and any(path.startswith(prefix) for prefix in prefixes)


def _role_write_path_allowed(role: str, path: str) -> bool:
    prefixes = ROLE_WRITE_ALLOWED_PREFIXES.get(role, [])
    return bool(prefixes) and any(path.startswith(prefix) for prefix in prefixes)


def _git_branch_file_paths(repo: Path, branch: str) -> list[str]:
    r = subprocess.run(
        ["git", "-C", str(repo), "ls-tree", "-r", "--name-only", branch],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        msg = (r.stderr or r.stdout or "").strip()
        raise RuntimeError(f"git ls-tree: {msg}")
    return sorted(
        {line.strip() for line in (r.stdout or "").splitlines() if line.strip()}
    )


def _bounded_role_memory_files(repo: Path, role: str) -> list[str]:
    return [
        path
        for path in _git_branch_file_paths(repo, role)
        if _role_memory_path_allowed(role, path) and not path.endswith("/.gitkeep")
    ]


def _git_blob_size(repo: Path, branch: str, path: str) -> int:
    r = subprocess.run(
        ["git", "-C", str(repo), "cat-file", "-s", f"{branch}:{path}"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        raise FileNotFoundError(path)
    try:
        return int((r.stdout or "0").strip() or 0)
    except ValueError:
        return 0


def _git_show_text(repo: Path, branch: str, path: str, max_bytes: int) -> str:
    size = _git_blob_size(repo, branch, path)
    if size > max_bytes:
        raise ValueError(f"file too large: {size} bytes exceeds {max_bytes} byte limit")
    r = subprocess.run(
        ["git", "-C", str(repo), "show", f"{branch}:{path}"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        raise FileNotFoundError(path)
    return r.stdout or ""


def _memory_tree_for_role(repo: Path, role: str) -> tuple[list[TreeNode], bool, int]:
    all_files = _bounded_role_memory_files(repo, role)
    truncated = len(all_files) > MEMORY_TREE_MAX_FILES
    files = all_files[:MEMORY_TREE_MAX_FILES]
    dir_paths: set[str] = set()
    for f in files:
        parts = [p for p in f.split("/") if p]
        for i in range(1, len(parts)):
            dir_paths.add("/".join(parts[:i]))
    all_paths = set(files) | dir_paths
    ordered_paths = sorted(all_paths, key=lambda p: (p.count("/"), p.lower()))
    nodes: dict[str, TreeNode] = {}
    for path in ordered_paths:
        is_dir = path in dir_paths
        nodes[path] = {
            "name": path.rsplit("/", 1)[-1],
            "path": path,
            "type": "directory" if is_dir else "file",
            "children": [],
        }
    roots: list[TreeNode] = []
    for path in ordered_paths:
        node = nodes[path]
        if "/" in path:
            parent = nodes.get(path.rsplit("/", 1)[0])
            if parent is None:
                roots.append(node)
            else:
                cast(list[TreeNode], parent["children"]).append(node)
        else:
            roots.append(node)
    _sort_tree_nodes(roots)
    return roots, truncated, len(all_files)


def _memory_status_for_role(bear_id: str, role: str) -> ResponseDict:
    if role not in ROLE_BRANCHES:
        raise ValueError("valid role is required")
    canonical = ensure_canonical_repo(bear_id)
    tip = _branch_tip(canonical, role)
    files = _bounded_role_memory_files(canonical, role) if tip else []
    by_kind: dict[str, int] = {kind: 0 for kind in MEMORY_ENTRY_KIND_DIRS}
    for path in files:
        parts = path.split("/")
        if len(parts) >= 3:
            for kind, kind_dir in MEMORY_ENTRY_KIND_DIRS.items():
                if parts[0] == role and parts[1] == kind_dir:
                    by_kind[kind] += 1
                    break
    data = _read_view_registry()
    views_obj = data.get("views", {})
    view_count = 0
    if isinstance(views_obj, dict):
        view_count = sum(
            1
            for record_obj in views_obj.values()
            if isinstance(record_obj, dict)
            and str(record_obj.get("bear_id")) == bear_id
            and str(record_obj.get("role")) == role
        )
    recent = [
        event
        for event in recent_activity(None, 100)
        if str(event.get("bear_id") or "") == bear_id
        and str(event.get("role") or "") == role
    ][-20:]
    return {
        "ok": True,
        "bear_id": bear_id,
        "role": role,
        "canonical_repo": str(canonical),
        "canonical_branch": role,
        "canonical_tip": tip,
        "allowed_prefixes": _role_memory_prefixes(role),
        "file_count": len(files),
        "entry_count_by_kind": by_kind,
        "registered_view_count": view_count,
        "recent_activity": recent,
    }


def _memory_tree_response(bear_id: str, role: str) -> ResponseDict:
    if role not in ROLE_BRANCHES:
        raise ValueError("valid role is required")
    canonical = ensure_canonical_repo(bear_id)
    tree, truncated, total_file_count = _memory_tree_for_role(canonical, role)
    return {
        "ok": True,
        "bear_id": bear_id,
        "role": role,
        "canonical_tip": _branch_tip(canonical, role),
        "files": tree,
        "truncated": truncated,
        "total_file_count": total_file_count,
        "limit": MEMORY_TREE_MAX_FILES,
    }


def _memory_file_response(bear_id: str, role: str, raw_path: str) -> ResponseDict:
    if role not in ROLE_BRANCHES:
        raise ValueError("valid role is required")
    rel_path = _normalize_memory_path(raw_path)
    if not _role_memory_path_allowed(role, rel_path):
        raise PermissionError(f"path is not allowed for role {role}: {rel_path}")
    canonical = ensure_canonical_repo(bear_id)
    content = _git_show_text(canonical, role, rel_path, MEMORY_FILE_MAX_BYTES)
    return {
        "ok": True,
        "bear_id": bear_id,
        "role": role,
        "path": rel_path,
        "canonical_tip": _branch_tip(canonical, role),
        "content": content,
        "size_bytes": len(content.encode("utf-8")),
    }


def _extract_frontmatter_string(content: str, key: str) -> str | None:
    match = re.search(rf"^\s*{re.escape(key)}:\s*(.+?)\s*$", content, re.MULTILINE)
    if not match:
        return None
    value = match.group(1).strip()
    if len(value) >= 2 and value[0] == '"' and value[-1] == '"':
        try:
            parsed = json.loads(value)
            return str(parsed)
        except json.JSONDecodeError:
            return value.strip('"')
    return value


def _snippet_for_match(content: str, query: str, max_len: int = 240) -> str:
    lower = content.lower()
    pos = lower.find(query.lower())
    if pos < 0:
        return content[:max_len].replace("\n", " ").strip()
    start = max(0, pos - 80)
    end = min(len(content), pos + len(query) + 160)
    snippet = content[start:end].replace("\n", " ").strip()
    if start > 0:
        snippet = "..." + snippet
    if end < len(content):
        snippet += "..."
    return snippet[: max_len + 6]


def _snippet_for_path_match(path: str, max_len: int = 240) -> str:
    snippet = f"Path: {path}"
    return snippet[:max_len]


def _memory_search_response(
    bear_id: str, role: str, query: str, limit: int
) -> ResponseDict:
    if role not in ROLE_BRANCHES:
        raise ValueError("valid role is required")
    query = query.strip()
    if not query:
        raise ValueError("query is required")
    limit = max(1, min(limit, MEMORY_SEARCH_MAX_RESULTS))
    canonical = ensure_canonical_repo(bear_id)
    files = _bounded_role_memory_files(canonical, role)
    results: list[ResponseDict] = []
    scanned = 0
    query_lower = query.lower()
    for path in files:
        if len(results) >= limit:
            break
        path_score = path.lower().count(query_lower)
        content = ""
        content_score = 0
        try:
            size = _git_blob_size(canonical, role, path)
            if size <= MEMORY_SEARCH_FILE_MAX_BYTES:
                content = _git_show_text(
                    canonical, role, path, MEMORY_SEARCH_FILE_MAX_BYTES
                )
                scanned += 1
                content_score = content.lower().count(query_lower)
        except (FileNotFoundError, ValueError):
            continue
        score = path_score + content_score
        if score <= 0:
            continue
        results.append(
            {
                "path": path,
                "title": _extract_frontmatter_string(content, "title")
                if content
                else None,
                "kind": _extract_frontmatter_string(content, "kind")
                if content
                else None,
                "entry_id": _extract_frontmatter_string(content, "entry_id")
                if content
                else None,
                "score": score,
                "snippet": _snippet_for_match(content, query)
                if content_score > 0
                else _snippet_for_path_match(path),
                "size_bytes": size,
            }
        )
    results.sort(key=lambda item: (-int(item.get("score") or 0), str(item.get("path"))))
    return {
        "ok": True,
        "bear_id": bear_id,
        "role": role,
        "query": query,
        "canonical_tip": _branch_tip(canonical, role),
        "results": results[:limit],
        "result_count": len(results[:limit]),
        "scanned_file_count": scanned,
        "limit": limit,
    }


def _read_view_registry() -> ViewRegistry:
    if not VIEW_REGISTRY_PATH.exists():
        return {"version": 1, "views": {}}
    try:
        data = json.loads(VIEW_REGISTRY_PATH.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            return {"version": 1, "views": {}}
        registry = cast(ViewRegistry, data)
        registry.setdefault("version", 1)
        registry.setdefault("views", {})
        return registry
    except (OSError, json.JSONDecodeError):
        return {"version": 1, "views": {}}


def _write_view_registry(data: ViewRegistry) -> None:
    VIEW_REGISTRY_PATH.parent.mkdir(parents=True, exist_ok=True)
    tmp = VIEW_REGISTRY_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
    tmp.replace(VIEW_REGISTRY_PATH)


def _view_record(agent_id: str) -> ViewRecord | None:
    views = _read_view_registry().get("views", {})
    if not isinstance(views, dict):
        return None
    record = views.get(agent_id)
    return cast(ViewRecord, record) if isinstance(record, dict) else None


def _canonical_repo_path(bear_id: str) -> Path:
    return BEARS_CANONICAL_BASE / f"{bear_id}.git"


def _view_repo_path(org_id: str, agent_id: str) -> Path:
    return MEMFS_BASE / org_id / agent_id / "repo.git"


def _repo_is_quarantined(repo: Path) -> bool:
    return (repo / "BEARS_QUARANTINED").exists()


def _update_view_record(agent_id: str, **fields: object) -> None:
    data = _read_view_registry()
    record = data.setdefault("views", {}).get(agent_id)
    if not record:
        return
    record.update(fields)
    _write_view_registry(data)


def _set_quarantine(repo: Path, reason: str) -> None:
    repo.mkdir(parents=True, exist_ok=True)
    (repo / "BEARS_QUARANTINED").write_text(reason, encoding="utf-8")


def _archive_view_repo(view_repo: Path, label: str) -> Path | None:
    if not view_repo.exists():
        return None
    archive = view_repo.with_name(f"{view_repo.name}.{label}.{int(time.time())}")
    shutil.move(str(view_repo), str(archive))
    return archive


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


def git_head_info(repo: Path) -> ResponseDict:
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


def _sort_tree_nodes(nodes: list[TreeNode]) -> None:
    nodes.sort(
        key=lambda n: (
            0 if n.get("type") == "directory" else 1,
            str(n.get("name", "")).lower(),
        )
    )
    for n in nodes:
        children = n.get("children")
        if isinstance(children, list) and children:
            _sort_tree_nodes(cast(list[TreeNode], children))


def git_repository_file_tree(repo: Path) -> list[TreeNode]:
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

    nodes: dict[str, TreeNode] = {}
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

    roots: list[TreeNode] = []
    for path in ordered_paths:
        node = nodes[path]
        if "/" in path:
            parent_path = path.rsplit("/", 1)[0]
            parent = nodes.get(parent_path)
            if parent is None:
                roots.append(node)
            else:
                cast(list[TreeNode], parent["children"]).append(node)
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


def repository_status(agent_id: str, org_id: str) -> ResponseDict:
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
            "talk": [
                "talk/tasks",
                "talk/notes",
                "talk/logs",
                "talk/decisions",
                "talk/reflections",
                "talk/scratch",
                "talk/summaries",
            ],
            "pair": [
                "pair/tasks",
                "pair/notes",
                "pair/logs",
                "pair/decisions",
                "pair/reflections",
                "pair/scratch",
                "pair/summaries",
            ],
            "curate": [
                "curate/notes",
                "curate/logs",
                "curate/decisions",
                "curate/reflections",
                "curate/scratch",
                "curate/summaries",
                "core/tasks",
                "core/results",
            ],
            "work": [
                "work/results",
                "work/notes",
                "work/logs",
                "work/decisions",
                "work/reflections",
                "work/scratch",
                "work/summaries",
            ],
            "watch": [
                "watch/observations",
                "watch/subscriptions",
                "watch/notes",
                "watch/logs",
                "watch/decisions",
                "watch/reflections",
                "watch/scratch",
                "watch/summaries",
            ],
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


def _force_reset_view_to_canonical(
    view_repo: Path, canonical_repo: Path, role: str
) -> None:
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
        f"+refs/heads/{role}:refs/heads/main",
    )
    _git("--git-dir", str(view_repo), "symbolic-ref", "HEAD", "refs/heads/main")
    _git("--git-dir", str(view_repo), "config", "http.receivepack", "true")
    _clear_quarantine(view_repo)


def _forward_view_to_canonical(
    record: ViewRecord, old_tip: str | None = None
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
        record["last_reconciled_at"] = time.time()
        record["last_reconcile_status"] = "ok"
        record["drift_count"] = int(record.get("drift_count") or 0)
        return "already_current", None
    if canonical_tip and _is_ancestor(view, view_tip, canonical_tip):
        _force_reset_view_to_canonical(view, canonical, role)
        record["last_reconciled_at"] = time.time()
        record["last_reconcile_status"] = "fast_forwarded_view"
        return "fast_forwarded_view", None
    if canonical_tip and not _is_ancestor(view, canonical_tip, view_tip):
        reason = f"view and canonical diverged for role {role}: canonical={canonical_tip}, view={view_tip}"
        _set_quarantine(view, reason)
        record["drift_count"] = int(record.get("drift_count") or 0) + 1
        record["last_reconciled_at"] = time.time()
        record["last_reconcile_status"] = "quarantined"
        record["last_reconcile_reason"] = reason
        log_activity(
            "view_quarantined",
            bear_id=bear_id,
            role=role,
            agent_id=agent_id,
            reason=reason,
        )
        return "quarantined", reason
    paths = _changed_paths(view, canonical_tip, view_tip)
    ok, bad_path = _write_paths_allowed(role, paths)
    if not ok:
        reason = f"canonical would reject role {role} path {bad_path}"
        _set_quarantine(view, reason)
        record["drift_count"] = int(record.get("drift_count") or 0) + 1
        record["last_reconciled_at"] = time.time()
        record["last_reconcile_status"] = "quarantined"
        record["last_reconcile_reason"] = reason
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
        record["drift_count"] = int(record.get("drift_count") or 0) + 1
        record["last_reconciled_at"] = time.time()
        record["last_reconcile_status"] = "quarantined"
        record["last_reconcile_reason"] = reason
        log_activity(
            "forward_failed_quarantined",
            bear_id=bear_id,
            role=role,
            agent_id=agent_id,
            reason=reason,
        )
        return "quarantined", reason
    record["last_reconciled_at"] = time.time()
    record["last_reconcile_status"] = "forwarded"
    record["last_successful_forward_at"] = record["last_reconciled_at"]
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


def _slugify_note_title(title: str) -> str:
    slug = re.sub(r"[^a-zA-Z0-9]+", "-", title.strip().lower()).strip("-")
    return slug[:80] or "note"


def _yaml_quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def _frontmatter_list(
    lines: list[str], key: str, values: list[str], limit: int
) -> None:
    clean = [str(value).strip() for value in values if str(value).strip()]
    if not clean:
        return
    lines.append(f"{key}:")
    for value in clean[:limit]:
        lines.append(f"  - {_yaml_quote(value)}")


def _frontmatter_json(lines: list[str], key: str, value: object) -> None:
    if value is None or value == {} or value == []:
        return
    lines.append(f"{key}: |")
    for line in json.dumps(
        value, ensure_ascii=False, sort_keys=True, indent=2
    ).splitlines():
        lines.append(f"  {line}")


def _sync_role_views_after_canonical_write(
    bear_id: str, role: str, org_id: str, canonical: Path, status: str
) -> None:
    data = _read_view_registry()
    views = data.setdefault("views", {})
    if not isinstance(views, dict):
        views = {}
        data["views"] = views
    for agent_id, record_obj in views.items():
        if not isinstance(record_obj, dict):
            continue
        record = cast(ViewRecord, record_obj)
        if str(record.get("bear_id")) == bear_id and str(record.get("role")) == role:
            try:
                ensure_view_repo(agent_id, str(record.get("org_id") or org_id), record)
                _force_reset_view_to_canonical(
                    Path(record["view_repo"]), canonical, role
                )
                record["last_reconciled_at"] = time.time()
                record["last_reconcile_status"] = status
            except Exception as e:
                record["last_reconcile_status"] = f"{status}_failed"
                record["last_reconcile_reason"] = str(e)
            views[agent_id] = record
    _write_view_registry(data)


def _write_role_memory_entry(
    bear_id: str, role: str, body: JSONDict, org_id: str
) -> ResponseDict:
    if role not in ROLE_BRANCHES:
        raise ValueError("valid role is required")
    if role != "pair":
        raise ValueError("role memory entry writes are currently enabled only for pair")
    kind = str(body.get("kind") or "note").strip().lower()
    kind_dir = MEMORY_ENTRY_KIND_DIRS.get(kind)
    if not kind_dir:
        raise ValueError(
            "kind must be one of note, log, decision, reflection, scratch, summary"
        )
    title = str(body.get("title") or "").strip()
    entry_body = str(body.get("body") or "").strip()
    if not title:
        raise ValueError("title is required")
    if not entry_body:
        raise ValueError("body is required")
    tags = [str(tag).strip() for tag in body.get("tags") or [] if str(tag).strip()]
    refs = [str(ref).strip() for ref in body.get("refs") or [] if str(ref).strip()]
    lifecycle = str(body.get("lifecycle") or "active").strip() or "active"
    entry_id = f"mem_{uuid.uuid4().hex}"
    rel_path = f"{role}/{kind_dir}/{entry_id}.md"
    if not _role_write_path_allowed(role, rel_path):
        raise ValueError(f"role path policy rejected {rel_path}")
    canonical = ensure_canonical_repo(bear_id)
    timestamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "w"
        _git("clone", "--branch", role, str(canonical), str(work))
        _git("-C", str(work), "config", "user.name", "BEARS Den")
        _git("-C", str(work), "config", "user.email", "den@bears.local")
        target = work / rel_path
        target.parent.mkdir(parents=True, exist_ok=True)
        metadata = [
            "---",
            f"entry_id: {_yaml_quote(entry_id)}",
            f"kind: {_yaml_quote(kind)}",
            f"title: {_yaml_quote(title)}",
            f"role: {_yaml_quote(role)}",
            f"bear_id: {_yaml_quote(bear_id)}",
            f"path: {_yaml_quote(rel_path)}",
            f"lifecycle: {_yaml_quote(lifecycle)}",
            f"created_at: {_yaml_quote(timestamp)}",
        ]
        for key in [
            "author",
            "conversation_id",
            "session_id",
            "acp_session_id",
            "conversation_selection",
            "runtime_target",
            "role_agent_id",
            "agent_role",
            "request_id",
            "provenance",
        ]:
            value = str(body.get(key) or "").strip()
            if value:
                metadata.append(f"{key}: {_yaml_quote(value)}")
        _frontmatter_list(metadata, "tags", tags, 50)
        _frontmatter_list(metadata, "refs", refs, 50)
        _frontmatter_json(metadata, "source_json", body.get("source"))
        _frontmatter_json(metadata, "provenance_json", body.get("provenance_json"))
        metadata.extend(["---", "", f"# {title}", "", entry_body.rstrip(), ""])
        target.write_text("\n".join(metadata), encoding="utf-8")
        _git("add", rel_path, cwd=work)
        _git("commit", "-m", f"{role} {kind}: {title[:80]}", cwd=work)
        _git("push", "origin", f"HEAD:refs/heads/{role}", cwd=work)
    canonical_tip = _branch_tip(canonical, role)
    _sync_role_views_after_canonical_write(
        bear_id, role, org_id, canonical, "memory_entry_write_view_reset"
    )
    log_activity(
        "role_memory_entry_written",
        bear_id=bear_id,
        role=role,
        kind=kind,
        entry_id=entry_id,
        path=rel_path,
        canonical_tip=canonical_tip,
    )
    return {
        "ok": True,
        "bear_id": bear_id,
        "role": role,
        "kind": kind,
        "entry_id": entry_id,
        "path": rel_path,
        "commit": canonical_tip,
        "canonical_tip": canonical_tip,
    }


def _delete_role_memory_entries(
    bear_id: str, role: str, body: JSONDict, org_id: str
) -> ResponseDict:
    if role not in ROLE_BRANCHES:
        raise ValueError("valid role is required")
    paths_raw = body.get("paths") or []
    if not isinstance(paths_raw, list):
        raise ValueError("paths must be an array")
    paths: list[str] = []
    for raw in paths_raw:
        rel_path = _normalize_memory_path(str(raw))
        if not _role_write_path_allowed(role, rel_path):
            raise PermissionError(f"path is not allowed for role {role}: {rel_path}")
        if not rel_path.endswith(".md"):
            raise ValueError(f"only Markdown memory files can be deleted: {rel_path}")
        paths.append(rel_path)
    paths = sorted(set(paths))
    if not paths:
        raise ValueError("at least one path is required")
    if len(paths) > 100:
        raise ValueError("cannot delete more than 100 memory files at once")
    canonical = ensure_canonical_repo(bear_id)
    deleted: list[str] = []
    missing: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "w"
        _git("clone", "--branch", role, str(canonical), str(work))
        _git("-C", str(work), "config", "user.name", "BEARS Den")
        _git("-C", str(work), "config", "user.email", "den@bears.local")
        for rel_path in paths:
            target = work / rel_path
            if target.exists() and target.is_file():
                target.unlink()
                deleted.append(rel_path)
            else:
                missing.append(rel_path)
        if not deleted:
            return {
                "ok": True,
                "bear_id": bear_id,
                "role": role,
                "deleted": [],
                "missing": missing,
                "commit": _branch_tip(canonical, role),
                "canonical_tip": _branch_tip(canonical, role),
                "message": "No selected files existed in canonical memory.",
            }
        _git("add", "-A", cwd=work)
        _git("commit", "-m", f"delete {role} memory entries ({len(deleted)})", cwd=work)
        _git("push", "origin", f"HEAD:refs/heads/{role}", cwd=work)
    canonical_tip = _branch_tip(canonical, role)
    _sync_role_views_after_canonical_write(
        bear_id, role, org_id, canonical, "memory_entry_delete_view_reset"
    )
    log_activity(
        "role_memory_entries_deleted",
        bear_id=bear_id,
        role=role,
        deleted_count=len(deleted),
        canonical_tip=canonical_tip,
    )
    return {
        "ok": True,
        "bear_id": bear_id,
        "role": role,
        "deleted": deleted,
        "missing": missing,
        "commit": canonical_tip,
        "canonical_tip": canonical_tip,
    }


def _write_role_note(
    bear_id: str, role: str, body: JSONDict, org_id: str
) -> ResponseDict:
    note_body = dict(body)
    note_body["kind"] = "note"
    return _write_role_memory_entry(bear_id, role, note_body, org_id)


def ensure_view_repo(agent_id: str, org_id: str, record: ViewRecord) -> Path:
    canonical = ensure_canonical_repo(str(record["bear_id"]))
    view = _view_repo_path(org_id, agent_id)
    record["canonical_repo"] = str(canonical)
    record["view_repo"] = str(view)
    role = str(record["role"])
    if not view.exists() or not _is_usable_bare_repo(view):
        if view.exists():
            shutil.rmtree(view, ignore_errors=True)
        _force_reset_view_to_canonical(view, canonical, role)
    else:
        _git("--git-dir", str(view), "symbolic-ref", "HEAD", "refs/heads/main")
        _git("--git-dir", str(view), "config", "http.receivepack", "true")
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

    def _read_json_body(self) -> JSONDict:
        body = json.loads(self._read_body().decode("utf-8") or "{}")
        if not isinstance(body, dict):
            raise ValueError("request body must be a JSON object")
        return cast(JSONDict, body)

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
            len(parts) >= 7
            and parts[:3] == ["v1", "management", "bears"]
            and parts[4] == "roles"
        ):
            bear_id = parts[3]
            role = parts[5]
            endpoint = parts[6]
            query = parse_qs(parsed.query)
            try:
                if endpoint == "memory-status" and len(parts) == 7:
                    self._send_json(200, _memory_status_for_role(bear_id, role))
                    return True
                if endpoint == "memory-tree" and len(parts) == 7:
                    self._send_json(200, _memory_tree_response(bear_id, role))
                    return True
                if endpoint == "memory-files":
                    raw_path = "/".join(parts[7:]) or (query.get("path") or [""])[0]
                    self._send_json(200, _memory_file_response(bear_id, role, raw_path))
                    return True
                if endpoint == "memory-search" and len(parts) == 7:
                    raw_limit = (
                        query.get("limit") or [str(MEMORY_SEARCH_MAX_RESULTS)]
                    )[0]
                    try:
                        limit = int(raw_limit)
                    except ValueError:
                        limit = MEMORY_SEARCH_MAX_RESULTS
                    self._send_json(
                        200,
                        _memory_search_response(
                            bear_id, role, (query.get("query") or [""])[0], limit
                        ),
                    )
                    return True
            except PermissionError as e:
                self._send_json(403, {"ok": False, "error": str(e)})
                return True
            except FileNotFoundError as e:
                self._send_json(404, {"ok": False, "error": str(e) or "not found"})
                return True
            except (ValueError, RuntimeError, subprocess.CalledProcessError) as e:
                self._send_json(400, {"ok": False, "error": str(e)})
                return True
        if (
            len(parts) == 6
            and parts[:3] == ["v1", "management", "bears"]
            and parts[4] == "roles"
        ):
            bear_id = parts[3]
            role = parts[5]
            data = _read_view_registry()
            rows: list[ResponseDict] = []
            views = data.get("views", {})
            if not isinstance(views, dict):
                views = {}
            for record_obj in views.values():
                if not isinstance(record_obj, dict):
                    continue
                record = cast(ViewRecord, record_obj)
                if (
                    str(record.get("bear_id")) == bear_id
                    and str(record.get("role")) == role
                ):
                    rows.append(self._view_health(record))
            self._send_json(200, {"bear_id": bear_id, "role": role, "views": rows})
            return True
        return False

    def _view_health(self, record: ViewRecord) -> ResponseDict:
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
        merge_base = (
            _merge_base(view, canonical_tip, view_tip) if view.exists() else None
        )
        view_ahead_by = (
            _rev_count(view, f"{canonical_tip}..{view_tip}")
            if canonical_tip and view_tip and view.exists()
            else None
        )
        canonical_ahead_by = (
            _rev_count(view, f"{view_tip}..{canonical_tip}")
            if canonical_tip and view_tip and view.exists()
            else None
        )
        state = "ok"
        recommended_action = None
        if quarantined:
            state = "quarantined"
            recommended_action = "Inspect diagnostic, then run reconcile or an operator override (canonical-wins, recreate-view, view-wins, or clear-quarantine)."
        elif not canonical.exists():
            state = "missing_canonical"
            recommended_action = "Restore or initialize canonical Bear repo before accepting role memory writes."
        elif not view.exists():
            state = "missing_view"
            recommended_action = (
                "Run reconcile to recreate the role view from canonical."
            )
        elif canonical_tip != view_tip:
            state = "drift"
            recommended_action = "Run reconcile; sidecar will fast-forward, forward acceptable commits, or quarantine if unsafe."
        return {
            **record,
            "state": state,
            "canonical_exists": canonical.exists(),
            "view_exists": view.exists(),
            "canonical_tip": canonical_tip,
            "view_tip": view_tip,
            "merge_base": merge_base,
            "view_ahead_by": view_ahead_by,
            "canonical_ahead_by": canonical_ahead_by,
            "drift_count": int(record.get("drift_count") or 0),
            "last_successful_forward_at": record.get("last_successful_forward_at"),
            "last_reconciled_at": record.get("last_reconciled_at"),
            "last_reconcile_status": record.get("last_reconcile_status"),
            "quarantined": quarantined,
            "diagnostic": reason or record.get("last_reconcile_reason"),
            "recommended_action": recommended_action,
        }

    def _require_override_reason(self, body: JSONDict, action: str) -> str:
        reason = str(body.get("reason") or "").strip()
        confirm = str(body.get("confirm") or "").strip()
        if not reason:
            raise ValueError("operator override requires non-empty reason")
        if action in {"view-wins", "clear-quarantine-force"} and confirm != action:
            raise ValueError(f"operator override requires confirm='{action}'")
        return reason

    def _operator_override(
        self, agent_id: str, action: str, body: JSONDict
    ) -> ResponseDict:
        record = _view_record(agent_id)
        if not record:
            raise KeyError("view_not_registered")
        role = str(record["role"])
        bear_id = str(record["bear_id"])
        canonical = Path(record["canonical_repo"])
        view = Path(record["view_repo"])
        old_canonical_tip = _branch_tip(canonical, role) if canonical.exists() else None
        old_view_tip = _branch_tip(view, "main") if view.exists() else None
        reason = self._require_override_reason(
            body, action if action != "clear-quarantine" else "clear-quarantine"
        )

        archived_view = None
        if action == "canonical-wins":
            _force_reset_view_to_canonical(view, canonical, role)
            status = "canonical_wins"
        elif action == "recreate-view":
            archived_view = _archive_view_repo(view, "replaced")
            _force_reset_view_to_canonical(view, canonical, role)
            status = "view_recreated"
        elif action == "view-wins":
            if not old_view_tip:
                raise ValueError("view main branch missing; cannot apply view-wins")
            paths = _changed_paths(view, old_canonical_tip, old_view_tip)
            ok, bad_path = _write_paths_allowed(role, paths)
            if not ok and body.get("allow_policy_violation") is not True:
                raise ValueError(
                    f"view-wins would violate role path policy at {bad_path}; set allow_policy_violation=true only if policy is known wrong"
                )
            _git(
                "--git-dir",
                str(canonical),
                "fetch",
                str(view),
                f"+refs/heads/main:refs/heads/{role}",
            )
            _clear_quarantine(view)
            status = "view_wins"
        elif action == "clear-quarantine":
            current_canonical_tip = (
                _branch_tip(canonical, role) if canonical.exists() else None
            )
            current_view_tip = _branch_tip(view, "main") if view.exists() else None
            force = body.get("force") is True
            if current_canonical_tip != current_view_tip and not force:
                raise ValueError(
                    "cannot clear quarantine while canonical and view tips differ; reconcile or use force with confirm='clear-quarantine-force'"
                )
            if force:
                self._require_override_reason(body, "clear-quarantine-force")
            _clear_quarantine(view)
            status = "quarantine_cleared"
        else:
            raise ValueError(f"unknown override action: {action}")

        new_canonical_tip = _branch_tip(canonical, role) if canonical.exists() else None
        new_view_tip = _branch_tip(view, "main") if view.exists() else None
        record["last_override_action"] = action
        record["last_override_reason"] = reason
        record["last_override_at"] = time.time()
        record["last_override_old_canonical_tip"] = old_canonical_tip
        record["last_override_old_view_tip"] = old_view_tip
        record["last_override_new_canonical_tip"] = new_canonical_tip
        record["last_override_new_view_tip"] = new_view_tip
        data = _read_view_registry()
        data.setdefault("views", {})[agent_id] = record
        _write_view_registry(data)
        log_activity(
            "operator_override",
            agent_id=agent_id,
            bear_id=bear_id,
            role=role,
            action=action,
            reason=reason,
            old_canonical_tip=old_canonical_tip,
            old_view_tip=old_view_tip,
            new_canonical_tip=new_canonical_tip,
            new_view_tip=new_view_tip,
            archived_view=str(archived_view) if archived_view else None,
        )
        return {
            "ok": True,
            "status": status,
            "action": action,
            "archived_view": str(archived_view) if archived_view else None,
            "old_canonical_tip": old_canonical_tip,
            "old_view_tip": old_view_tip,
            "new_canonical_tip": new_canonical_tip,
            "new_view_tip": new_view_tip,
            "view": self._view_health(record),
        }

    def _try_view_management_post(self) -> bool:
        if self.command != "POST":
            return False
        parsed = urlparse(self.path)
        parts = [p for p in parsed.path.strip("/").split("/") if p]
        if (
            len(parts) == 6
            and parts[:3] == ["v1", "management", "views"]
            and parts[4] == "override"
        ):
            agent_id = parts[3]
            action = parts[5]
            try:
                body = self._read_json_body()
                self._send_json(200, self._operator_override(agent_id, action, body))
                return True
            except KeyError as e:
                self._send_json(404, {"ok": False, "error": str(e).strip("'")})
                return True
            except Exception as e:
                self._send_json(400, {"ok": False, "error": str(e)})
                return True

        if parts == ["v1", "management", "views", "register"]:
            try:
                body = self._read_json_body()
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
                record: ViewRecord = {
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
            len(parts) == 7
            and parts[:3] == ["v1", "management", "bears"]
            and parts[4] == "roles"
            and parts[6] in {"memory-entries", "notes", "memory-delete"}
        ):
            bear_id = parts[3]
            role = parts[5]
            try:
                body = self._read_json_body()
                org_id = resolve_org_id(self.headers.get("X-Organization-Id"))
                if parts[6] == "notes":
                    self._send_json(200, _write_role_note(bear_id, role, body, org_id))
                elif parts[6] == "memory-delete":
                    self._send_json(
                        200, _delete_role_memory_entries(bear_id, role, body, org_id)
                    )
                else:
                    self._send_json(
                        200, _write_role_memory_entry(bear_id, role, body, org_id)
                    )
                return True
            except Exception as e:
                self._send_json(400, {"ok": False, "error": str(e)})
                return True

        if parts == ["v1", "management", "views", "reconcile-all"]:
            try:
                self._send_json(
                    200, {"ok": True, "summary": reconcile_all_views_once()}
                )
                return True
            except Exception as e:
                self._send_json(500, {"ok": False, "error": str(e)})
                return True

        if (
            len(parts) == 5
            and parts[:3] == ["v1", "management", "views"]
            and parts[4] == "reconcile"
        ):
            agent_id = parts[3]
            view_record = _view_record(agent_id)
            if view_record is None:
                self._send_json(404, {"ok": False, "error": "view_not_registered"})
                return True
            try:
                ensure_view_repo(
                    agent_id,
                    str(view_record.get("org_id") or resolve_org_id(None)),
                    view_record,
                )
                status, reason = _forward_view_to_canonical(view_record)
                data = _read_view_registry()
                data.setdefault("views", {})[agent_id] = view_record
                _write_view_registry(data)
                self._send_json(
                    200,
                    {
                        "ok": status not in {"quarantined", "missing_view_tip"},
                        "status": status,
                        "reason": reason,
                        "view": self._view_health(view_record),
                    },
                )
                return True
            except Exception as e:
                self._send_json(
                    500,
                    {
                        "ok": False,
                        "error": str(e),
                        "view": self._view_health(view_record),
                    },
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

        env: dict[str, str] = {
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
        result: ResponseDict = {
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
                org_info: ResponseDict = {
                    "org": org_dir.name,
                    "agents": [],
                }
                for agent_dir in sorted(org_dir.iterdir()):
                    if not agent_dir.is_dir():
                        continue
                    repo = agent_dir / "repo.git"
                    info: ResponseDict = {
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
                    cast(list[ResponseDict], org_info["agents"]).append(info)
                cast(list[ResponseDict], result["repos"]).append(org_info)
        self._send_json(200, result)

    def do_POST(self) -> None:  # noqa: N802
        if self._try_view_management_post():
            return
        self._run_backend()

    def log_message(self, format: str, *args: object) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/health" and not LOG_HEALTHCHECKS:
            return
        print(f"[memfs-manager] {self.address_string()} {format % args}", flush=True)


def reconcile_all_views_once() -> ReconcileSummary:
    data = _read_view_registry()
    views = data.setdefault("views", {})
    if not isinstance(views, dict):
        views = {}
        data["views"] = views
    summary: ReconcileSummary = {
        "checked": 0,
        "ok": 0,
        "corrected": 0,
        "quarantined": 0,
        "errors": 0,
        "results": [],
    }
    for agent_id, record in list(views.items()):
        summary["checked"] += 1
        try:
            ensure_view_repo(
                agent_id, str(record.get("org_id") or resolve_org_id(None)), record
            )
            status, reason = _forward_view_to_canonical(record)
            views[agent_id] = record
            if status in {"already_current"}:
                summary["ok"] += 1
            elif status in {"forwarded", "fast_forwarded_view"}:
                summary["corrected"] += 1
            elif status == "quarantined":
                summary["quarantined"] += 1
            else:
                summary["errors"] += 1
            summary["results"].append(
                {"agent_id": agent_id, "status": status, "reason": reason}
            )
        except Exception as e:
            summary["errors"] += 1
            record["last_reconciled_at"] = time.time()
            record["last_reconcile_status"] = "error"
            record["last_reconcile_reason"] = str(e)
            views[agent_id] = record
            summary["results"].append(
                {"agent_id": agent_id, "status": "error", "reason": str(e)}
            )
            log_activity("reconcile_error", agent_id=agent_id, error=str(e))
    data["last_reconcile_all_at"] = time.time()
    data["last_reconcile_all_summary"] = summary
    _write_view_registry(data)
    log_activity(
        "reconcile_all", **{k: v for k, v in summary.items() if k != "results"}
    )
    return summary


def _scheduled_reconcile_loop() -> None:
    if RECONCILE_INTERVAL_SECONDS <= 0:
        return
    while True:
        time.sleep(RECONCILE_INTERVAL_SECONDS)
        try:
            reconcile_all_views_once()
        except Exception as e:
            log_activity("reconcile_loop_error", error=str(e))


if __name__ == "__main__":
    MEMFS_BASE.mkdir(parents=True, exist_ok=True)
    BEARS_CANONICAL_BASE.mkdir(parents=True, exist_ok=True)
    if RECONCILE_INTERVAL_SECONDS > 0:
        threading.Thread(target=_scheduled_reconcile_loop, daemon=True).start()
    print(
        f"[memfs-manager] listen http://{BIND}:{PORT} base={MEMFS_BASE} canonical_base={BEARS_CANONICAL_BASE} reconcile_interval={RECONCILE_INTERVAL_SECONDS}s",
        flush=True,
    )
    HTTPServer((BIND, PORT), GitHTTPHandler).serve_forever()
