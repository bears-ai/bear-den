#!/usr/bin/env python3
"""Check for newly broken documentation links and duplicate ADR identifiers.

Default comparison is against HEAD (including uncommitted edits). CI passes the
PR base SHA with --base. --all reports the full link backlog.
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


def adr_id_issues(paths, old_paths):
    """Reject new collisions, allowing renames and the existing collision backlog."""
    previous = Counter(ADR.fullmatch(path).group(1) for path in old_paths if ADR.fullmatch(path))
    current = Counter(ADR.fullmatch(path).group(1) for path in paths if ADR.fullmatch(path))
    return [
        f"ADR-{number}: newly introduced duplicate identifier; existing collisions are grandfathered"
        for number, count in sorted(current.items())
        if count > max(1, previous[number])
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="HEAD", help="Git revision to compare (default: HEAD)")
    parser.add_argument("--all", action="store_true", help="Fail on all existing broken links, not only new ones")
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
    duplicates = adr_id_issues(current, previous)
    for source, raw, reason in sorted(introduced):
        print(f"{source}: {reason}: {raw}")
    for issue in duplicates:
        print(issue)
    backlog = len(broken - introduced)
    print(f"Docs: {len(current)} pages; {len(introduced)} new link issues, {len(duplicates)} ADR-ID collisions; {backlog} grandfathered link issues")
    return bool(introduced or duplicates)


if __name__ == "__main__":
    raise SystemExit(main())
