import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).resolve().parents[1] / "git_memfs_server.py"


def run(*args, cwd=None, check=True):
    return subprocess.run(
        list(args),
        cwd=cwd,
        check=check,
        capture_output=True,
        text=True,
    )


@pytest.fixture()
def memfs(tmp_path, monkeypatch):
    monkeypatch.setenv("MEMFS_BASE", str(tmp_path / "memfs"))
    monkeypatch.setenv("BEARS_CANONICAL_MEMFS_BASE", str(tmp_path / "bears"))
    monkeypatch.setenv("MEMFS_VIEW_REGISTRY", str(tmp_path / "views.json"))
    monkeypatch.setenv("MEMFS_ACTIVITY_LOG", str(tmp_path / "activity.jsonl"))

    module_name = f"git_memfs_server_test_{os.getpid()}_{len(sys.modules)}"
    spec = importlib.util.spec_from_file_location(module_name, MODULE_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)

    module.MEMFS_BASE.mkdir(parents=True, exist_ok=True)
    module.BEARS_CANONICAL_BASE.mkdir(parents=True, exist_ok=True)
    return module


def register_view(memfs, agent_id="agent-talk", bear_id="bear-1", role="talk"):
    canonical = memfs.ensure_canonical_repo(bear_id)
    record = {
        "agent_id": agent_id,
        "bear_id": bear_id,
        "role": role,
        "org_id": memfs.DEFAULT_ORG,
        "canonical_repo": str(canonical),
        "canonical_branch": role,
        "view_repo": str(memfs._view_repo_path(memfs.DEFAULT_ORG, agent_id)),
        "registered_at": 1.0,
    }
    memfs.ensure_view_repo(agent_id, memfs.DEFAULT_ORG, record)
    data = memfs._read_view_registry()
    data.setdefault("views", {})[agent_id] = record
    memfs._write_view_registry(data)
    return record


def clone_view(memfs, agent_id="agent-talk", branch="main", tmp_path=None):
    work = tmp_path / f"work-{agent_id}"
    run(
        "git",
        "clone",
        "--branch",
        branch,
        str(memfs._view_repo_path(memfs.DEFAULT_ORG, agent_id)),
        str(work),
    )
    run("git", "config", "user.name", "MemFS Test", cwd=work)
    run("git", "config", "user.email", "memfs-test@example.test", cwd=work)
    return work


def commit_file(work, rel_path, content="hello"):
    path = work / rel_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    run("git", "add", ".", cwd=work)
    run("git", "commit", "-m", f"write {rel_path}", cwd=work)


def test_register_view_creates_canonical_and_view(memfs):
    record = register_view(memfs)
    canonical = Path(record["canonical_repo"])
    view = Path(record["view_repo"])

    assert canonical.exists()
    assert view.exists()
    assert memfs._branch_tip(canonical, "talk")
    assert memfs._branch_tip(canonical, "pair")
    assert memfs._branch_tip(canonical, "curate")
    assert memfs._branch_tip(canonical, "work")
    assert memfs._branch_tip(canonical, "watch")
    assert memfs._branch_tip(view, "main") == memfs._branch_tip(canonical, "talk")


def test_valid_role_push_forwards_to_canonical(memfs, tmp_path):
    record = register_view(memfs, agent_id="agent-talk", role="talk")
    work = clone_view(memfs, "agent-talk", tmp_path=tmp_path)
    commit_file(work, "talk/notes.md", "valid talk memory")
    run("git", "push", "origin", "HEAD:main", cwd=work)

    status, reason = memfs._forward_view_to_canonical(record)
    assert status == "forwarded"
    assert reason is None

    canonical = Path(record["canonical_repo"])
    assert memfs._branch_tip(canonical, "talk") == memfs._branch_tip(
        Path(record["view_repo"]), "main"
    )
    files = run(
        "git", "--git-dir", str(canonical), "ls-tree", "-r", "--name-only", "talk"
    ).stdout
    assert "talk/notes.md" in files


def test_invalid_role_push_quarantines_view(memfs, tmp_path):
    record = register_view(memfs, agent_id="agent-talk", role="talk")
    work = clone_view(memfs, "agent-talk", tmp_path=tmp_path)
    commit_file(work, "core/nope.md", "invalid cross-role write")
    run("git", "push", "origin", "HEAD:main", cwd=work)

    status, reason = memfs._forward_view_to_canonical(record)
    assert status == "quarantined"
    assert "core/nope.md" in reason
    assert memfs._repo_is_quarantined(Path(record["view_repo"]))

    canonical = Path(record["canonical_repo"])
    files = run(
        "git", "--git-dir", str(canonical), "ls-tree", "-r", "--name-only", "talk"
    ).stdout
    assert "core/nope.md" not in files


def test_reconcile_fast_forwards_view_when_behind_canonical(memfs, tmp_path):
    record = register_view(memfs, agent_id="agent-curate", role="curate")
    canonical = Path(record["canonical_repo"])
    view = Path(record["view_repo"])
    old_view_tip = memfs._branch_tip(view, "main")

    work = tmp_path / "canonical-work"
    run("git", "clone", "--branch", "curate", str(canonical), str(work))
    run("git", "config", "user.name", "MemFS Test", cwd=work)
    run("git", "config", "user.email", "memfs-test@example.test", cwd=work)
    commit_file(work, "core/shared.md", "curated fact")
    run("git", "push", "origin", "HEAD:curate", cwd=work)

    assert memfs._branch_tip(canonical, "curate") != old_view_tip
    status, reason = memfs._forward_view_to_canonical(record)
    assert status == "fast_forwarded_view"
    assert reason is None
    assert memfs._branch_tip(view, "main") == memfs._branch_tip(canonical, "curate")


def test_reconcile_recreates_missing_view_from_canonical(memfs):
    record = register_view(memfs, agent_id="agent-watch", role="watch")
    view = Path(record["view_repo"])
    canonical = Path(record["canonical_repo"])
    run("rm", "-rf", str(view))

    memfs.ensure_view_repo("agent-watch", memfs.DEFAULT_ORG, record)
    assert view.exists()
    assert memfs._branch_tip(view, "main") == memfs._branch_tip(canonical, "watch")


def test_registry_json_is_operator_inspectable(memfs):
    register_view(memfs, agent_id="agent-work", bear_id="bear-observable", role="work")
    data = json.loads(memfs.VIEW_REGISTRY_PATH.read_text(encoding="utf-8"))
    assert data["version"] == 1
    assert data["views"]["agent-work"]["bear_id"] == "bear-observable"
    assert data["views"]["agent-work"]["role"] == "work"
