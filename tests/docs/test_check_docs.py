import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/check-docs.py"
spec = importlib.util.spec_from_file_location("check_docs", SCRIPT)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class DocumentationLinkTests(unittest.TestCase):
    def test_link_regressions_are_ratcheted_against_the_base(self):
        old = {
            "docs/topic.md": "[old](missing.md)\n[guide](guide.md#heading)",
            "docs/guide.md": "# Heading",
        }
        current = dict(old, **{"docs/topic.md": old["docs/topic.md"] + "\n[new](missing-new.md)"})
        self.assertEqual(
            checker.link_issues(current) - checker.link_issues(old),
            {("docs/topic.md", "missing-new.md", "missing target")},
        )
        self.assertEqual(len(checker.link_issues(old)), 1)

    def test_deleted_link_targets_are_reported(self):
        contents = {"docs/topic.md": "[image](old.png)"}
        self.assertEqual(
            checker.link_issues(contents, lambda name: name == "docs/old.png"),
            set(),
        )
        self.assertEqual(
            checker.link_issues(contents, lambda name: False),
            {("docs/topic.md", "old.png", "missing target")},
        )

    def test_heading_changes_are_reported(self):
        old = {
            "docs/topic.md": "[guide](guide.md#heading)",
            "docs/guide.md": "# Heading",
        }
        current = dict(old, **{"docs/guide.md": "# Renamed"})
        self.assertEqual(
            checker.link_issues(current) - checker.link_issues(old),
            {("docs/topic.md", "guide.md#heading", "missing heading anchor")},
        )

    def test_external_urls_and_fenced_examples_are_ignored(self):
        self.assertEqual(
            list(checker.markdown_links(
                "[external](https://example.com)\n```md\n[example](missing.md)\n```\n"
                "[local](guide.md)"
            )),
            ["guide.md"],
        )

    def test_new_adr_cannot_duplicate_an_existing_identifier(self):
        old = {"docs/decisions/adr-0034-original.md"}
        current = old | {"docs/decisions/adr-0034-another.md"}
        issues = checker.adr_id_issues(current, old)
        self.assertEqual(len(issues), 1)
        self.assertIn("ADR-0034", issues[0])

    def test_two_new_adrs_cannot_share_an_identifier(self):
        current = {
            "docs/decisions/adr-0099-first.md",
            "docs/decisions/adr-0099-second.md",
        }
        self.assertEqual(len(checker.adr_id_issues(current, set())), 1)

    def test_unique_adrs_and_renames_are_allowed(self):
        old = {"docs/decisions/adr-0034-original.md"}
        current = {
            "docs/decisions/adr-0034-renamed.md",
            "docs/decisions/adr-0099-new.md",
        }
        self.assertEqual(checker.adr_id_issues(current, old), [])

    def test_existing_adr_collisions_are_grandfathered_but_cannot_grow(self):
        old = {
            "docs/decisions/adr-0034-first.md",
            "docs/decisions/adr-0034-second.md",
        }
        self.assertEqual(checker.adr_id_issues(old, old), [])
        current = old | {"docs/decisions/adr-0034-third.md"}
        self.assertEqual(len(checker.adr_id_issues(current, old)), 1)

    def run_check(self, current, previous, *args):
        output = io.StringIO()
        base_files = b"\0".join(path.encode() for path in previous)
        with (
            patch.object(checker, "working_docs", return_value=current),
            patch.object(checker, "revision_docs", return_value=previous),
            patch.object(checker, "git", return_value=base_files),
            patch("sys.argv", [str(SCRIPT), *args]),
            contextlib.redirect_stdout(output),
        ):
            failed = checker.main()
        return failed, output.getvalue()

    def test_new_pages_do_not_require_metadata_status_or_reciprocal_links(self):
        current = {
            "docs/topics/new-topic.md": "# Topic",
            "docs/roadmap/new-plan.md": "# Plan",
            "docs/decisions/adr-0099-new.md": "# Decision",
            "docs/archive/new-history.md": "# History",
            "docs/guides/new-guide.md": "# Guide",
            "services/den/docs/new-guide.md": "# Service guide",
        }
        failed, output = self.run_check(current, {})
        self.assertFalse(failed, output)
        self.assertNotIn("::warning::", output)

    def test_main_fails_on_new_link_and_adr_collisions(self):
        old = {"docs/decisions/adr-0099-first.md": "# First"}
        current = dict(old, **{
            "docs/decisions/adr-0099-second.md": "[missing](nonexistent-docs-test-target.md)",
        })
        failed, output = self.run_check(current, old)
        self.assertTrue(failed)
        self.assertIn("missing target", output)
        self.assertIn("ADR-0099", output)

    def test_all_reports_existing_link_backlog(self):
        contents = {"docs/example.md": "[missing](nonexistent-docs-test-target.md)"}
        failed, output = self.run_check(contents, contents)
        self.assertFalse(failed, output)
        self.assertIn("1 grandfathered link issues", output)
        failed, output = self.run_check(contents, contents, "--all")
        self.assertTrue(failed)
        self.assertIn("missing target", output)


if __name__ == "__main__":
    unittest.main()
