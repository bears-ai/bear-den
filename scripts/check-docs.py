#!/usr/bin/env python3
"""Ratchet documentation links and enforce structure on newly introduced pages.

Default comparison is against HEAD (including uncommitted edits). CI passes the
PR base or previous push SHA with --base. --all reports the full link backlog.
"""

import argparse
import re
import subprocess
from collections import Counter
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
ENTRY_POINTS = {
    "README.md", "CONTRIBUTING.md", "AGENTS.md", "services/den/AGENTS.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
}
LINK = re.compile(r"\[[^\]\n]+\]\((<[^>\n]+>|[^)\n]+)\)")
HEADING = re.compile(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$", re.MULTILINE)
FENCE = re.compile(r"(?m)^\s{0,3}(`{3,}|~{3,})[^\n]*\n.*?^\s{0,3}\1\s*$", re.DOTALL)
ADR = re.compile(r"^docs/decisions/adr-(\d{4})-[^/]+\.md$")
STATES = {"draft", "active", "completed", "superseded"}
CODE_TOPICS = {
    "docs/topics/bearwire-acp.md": (
        "services/den/crates/den-bearwire/", "tools/bear-armature/",
    ),
    "docs/topics/docket.md": ("services/den/crates/den-docket/",),
}


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True).stdout


def doc_path(name):
    return name.endswith(".md") and (
        name in ENTRY_POINTS or name.startswith(("docs/", "services/den/docs/"))
    )


def working_docs():
    names = git("ls-files", "--cached", "--others", "--exclude-standard", "-z")
    return {
        name: (ROOT / name).read_text(encoding="utf-8")
        for name in (item.decode() for item in names.split(b"\0") if item)
        if doc_path(name) and (ROOT / name).is_file()
    }


def revision_docs(revision):
    names = git("ls-tree", "-r", "--name-only", "-z", revision)
    return {
        name: git("show", f"{revision}:{name}").decode("utf-8")
        for name in (item.decode() for item in names.split(b"\0") if item)
        if doc_path(name)
    }


def markdown_links(text):
    text = FENCE.sub("", text)
    for match in LINK.finditer(text):
        raw = match.group(1).strip().strip("<>").split(' "', 1)[0]
        if raw and not raw.startswith(("//", "/")) and not urlsplit(raw).scheme:
            yield raw


def targets(source, text):
    for raw in markdown_links(text):
        url = urlsplit(raw)
        target = unquote(url.path)
        if not target:
            target = source
        else:
            resolved = (ROOT / Path(source).parent / target).resolve()
            target = str(resolved.relative_to(ROOT)) if resolved.is_relative_to(ROOT) else "<outside-repository>"
        yield target, unquote(url.fragment), raw


def anchors(text):
    cleaned = FENCE.sub("", text)
    seen = Counter()
    result = set(re.findall(r'<a\s+(?:id|name)=["\']([^"\']+)["\']', cleaned))
    for match in HEADING.finditer(cleaned):
        title = re.sub(r"<[^>]*>|[`*_]", "", match.group(1)).strip().lower()
        slug = re.sub(r"[^\w -]", "", title).replace(" ", "-")
        if slug:
            index = seen[slug]
            result.add(f"{slug}-{index}" if index else slug)
            seen[slug] += 1
    return result


def link_issues(contents, present=None):
    """Stable identities make previously broken links grandfatherable by revision."""
    if present is None:
        present = lambda path: (ROOT / path).exists()
    problems = set()
    heading_cache = {}
    for source, text in contents.items():
        for target, fragment, raw in targets(source, text):
            if target == "<outside-repository>":
                problems.add((source, raw, "outside repository"))
            elif target not in contents and not present(target):
                problems.add((source, raw, "missing target"))
            elif fragment and target in contents:
                if fragment not in heading_cache.setdefault(target, anchors(contents[target])):
                    problems.add((source, raw, "missing heading anchor"))
    return problems


def has_link(contents, source, dest):
    return source in contents and any(target == dest for target, _, _ in targets(source, contents[source]))


def field(text, name):
    match = re.search(rf"(?m)^\*\*{re.escape(name)}:\*\*\s*(.+)$", text)
    return match.group(1).strip() if match else None


def topic_link_issues(contents, path, text):
    home = field(text, "Topic")
    if not home:
        return [f"{path}: new page needs **Topic:** link"]
    candidates = [target for target, _, _ in targets(path, home)
                  if target.startswith("docs/topics/") and target in contents]
    if len(candidates) != 1:
        return [f"{path}: **Topic:** must link to one existing docs/topics/ page"]
    if not has_link(contents, candidates[0], path):
        return [f"{path}: link from owning topic {candidates[0]}"]
    return []


def structure_issues(contents, old_paths):
    issues = []
    for path, text in sorted(contents.items()):
        if path.startswith("services/den/docs/") and path not in old_paths:
            issues.extend(topic_link_issues(contents, path, text))
            continue
        if not path.startswith("docs/"):
            continue
        is_new = path not in old_paths
        if path.startswith("docs/topics/") and path.endswith(".md"):
            for name in ("Owner", "Scope", "Current as of", "Target", "Decisions"):
                if not field(text, name):
                    issues.append(f"{path}: missing **{name}:** provenance")
            current = field(text, "Current as of") or ""
            if not re.search(r"\b\d{4}-\d{2}-\d{2}\b", current) or "evidence:" not in current.lower():
                issues.append(f"{path}: Current as of needs a date and evidence")
            if not has_link(contents, "docs/README.md", path):
                issues.append(f"{path}: link from docs/README.md topic map")
        elif not is_new or path in {"docs/roadmap/README.md", "docs/README.md"}:
            continue
        elif path.startswith("docs/roadmap/archives/") or path.startswith("docs/archive/"):
            if (field(text, "Status") or "").lower() != "historical":
                issues.append(f"{path}: new archive needs **Status:** Historical")
        elif path.startswith("docs/roadmap/"):
            status = field(text, "Status")
            if not status or status.lower() not in STATES:
                issues.append(f"{path}: new plan needs **Status:** Draft|Active|Completed|Superseded")
            if not has_link(contents, "docs/roadmap/README.md", path):
                issues.append(f"{path}: index new plan in docs/roadmap/README.md")
            issues.extend(topic_link_issues(contents, path, text))
        elif ADR.fullmatch(path):
            if not field(text, "Status"):
                issues.append(f"{path}: new ADR needs **Status:**")
            if not has_link(contents, "docs/decisions/README.md", path):
                issues.append(f"{path}: index new ADR in docs/decisions/README.md")
            issues.extend(topic_link_issues(contents, path, text))
        else:
            issues.extend(topic_link_issues(contents, path, text))
    previous_ids = {ADR.fullmatch(path).group(1) for path in old_paths if ADR.fullmatch(path)}
    new_ids = Counter(ADR.fullmatch(path).group(1) for path in contents if ADR.fullmatch(path) and path not in old_paths)
    for number, count in new_ids.items():
        if number in previous_ids or count > 1:
            issues.append(f"ADR-{number}: newly introduced duplicate identifier; existing collisions are grandfathered")
    return issues


def documentation_impact_prompts(changed):
    return [topic for topic, prefixes in CODE_TOPICS.items()
            if any(path.startswith(prefixes) for path in changed) and topic not in changed]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="HEAD", help="Git revision to compare (default: HEAD)")
    parser.add_argument("--all", action="store_true", help="Fail on all existing broken links, not only new ones")
    parser.add_argument("--impact", action="store_true", help="Warn when mapped code changes lack a topic update")
    args = parser.parse_args()
    try:
        current = working_docs()
        previous = revision_docs(args.base)
    except (subprocess.CalledProcessError, UnicodeDecodeError) as error:
        parser.error(f"cannot load documentation from {args.base}: {error}")
    broken = link_issues(current)
    base_files = {item.decode() for item in git("ls-tree", "-r", "--name-only", "-z", args.base).split(b"\0") if item}
    def present_at_base(path):
        return path in base_files or any(name.startswith(path.rstrip("/") + "/") for name in base_files)
    introduced = broken if args.all else broken - link_issues(previous, present_at_base)
    structural = structure_issues(current, previous)
    for source, raw, reason in sorted(introduced):
        print(f"{source}: {reason}: {raw}")
    for issue in structural:
        print(issue)
    if args.impact:
        changed = {item.decode() for item in git("diff", "--name-only", "-z", args.base).split(b"\0") if item}
        for topic in documentation_impact_prompts(changed):
            print(f"::warning::Code for {topic} changed without a topic update; explain documentation impact in the PR")
    backlog = len(broken - introduced)
    print(f"Docs: {len(current)} pages; {len(introduced)} new link issues, {len(structural)} structure issues; {backlog} grandfathered link issues")
    return bool(introduced or structural)


if __name__ == "__main__":
    raise SystemExit(main())
