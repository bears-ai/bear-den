import importlib.util
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/check-docs.py"
spec = importlib.util.spec_from_file_location("check_docs", SCRIPT)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class DocumentationGuardTests(unittest.TestCase):
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
        deleted_asset = {"docs/topic.md": "[image](old.png)"}
        self.assertEqual(
            checker.link_issues(deleted_asset, lambda name: name == "docs/old.png"),
            set(),
        )
        self.assertEqual(len(checker.link_issues(deleted_asset, lambda name: False)), 1)

    def test_new_topic_needs_provenance_and_a_navigation_link(self):
        path = "docs/topics/new-topic.md"
        contents = {"docs/README.md": "[New topic](topics/new-topic.md)", path: "# Topic"}
        issues = checker.structure_issues(contents, {"docs/README.md"})
        self.assertTrue(any("Current as of" in issue for issue in issues), issues)
        self.assertFalse(any("link from docs/README.md" in issue for issue in issues), issues)

    def test_new_plan_requires_status_topic_and_index_and_adr_id_cannot_collide(self):
        old = {"docs/decisions/adr-0034-original.md"}
        contents = {
            "docs/roadmap/new-plan.md": "# Plan",
            "docs/decisions/adr-0034-another.md": "# ADR",
        }
        issues = checker.structure_issues(contents, old)
        self.assertTrue(any("new plan needs **Status:**" in issue for issue in issues), issues)
        self.assertTrue(any("index new plan" in issue for issue in issues), issues)
        self.assertTrue(any("duplicate identifier" in issue for issue in issues), issues)

    def test_changed_code_prompts_for_topic_review_without_blocking(self):
        changed = {"services/den/crates/den-bearwire/src/methods/run.rs"}
        self.assertEqual(checker.documentation_impact_prompts(changed), ["docs/topics/bearwire-acp.md"])
        changed.add("docs/topics/bearwire-acp.md")
        self.assertEqual(checker.documentation_impact_prompts(changed), [])

    def test_valid_plan_requires_real_topic_and_reciprocal_index(self):
        plan = "docs/roadmap/next.md"
        topic = "docs/topics/work.md"
        contents = {
            "docs/README.md": "[Work](topics/work.md)",
            "docs/roadmap/README.md": "[Next](next.md)",
            topic: "**Owner:** Work maintainers\n**Scope:** Work\n"
                   "**Current as of:** 2026-09-26; evidence: tests\n"
                   "**Target:** Work\n**Decisions:** none\n"
                   "[Next](../roadmap/next.md)",
            plan: "**Status:** Active\n**Topic:** [Work](../topics/work.md)",
        }
        self.assertEqual(checker.structure_issues(contents, set()), [])
        contents[plan] = "**Status:** Active\n**Topic:** Work"
        self.assertTrue(any("must link" in issue for issue in checker.structure_issues(contents, set())))


if __name__ == "__main__":
    unittest.main()
