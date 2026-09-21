import json
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from fixture_support import FIXTURES_ROOT, materialize_fixture  # noqa: E402
from retrieval_rules import (  # noqa: E402
    _match_strength,
    bm25_shortlist,
    run_ruleset,
)
from retrieval_replay import RULESET_ORDER, evaluate_fixture  # noqa: E402


COUNTERFACTUALS = RULESET_ORDER[1:]


class CounterfactualRuleTests(unittest.TestCase):
    def test_every_counterfactual_runs_on_every_fixture(self):
        fixtures = sorted(
            path.name
            for path in FIXTURES_ROOT.iterdir()
            if path.is_dir() and (path / "plan.md").is_file()
        )
        with tempfile.TemporaryDirectory() as temporary:
            for fixture in fixtures:
                root = materialize_fixture(fixture, Path(temporary))
                matrix = evaluate_fixture(root)
                current = matrix["current"]
                for ruleset in COUNTERFACTUALS:
                    with self.subTest(fixture=fixture, ruleset=ruleset):
                        result = matrix[ruleset]
                        self.assertGreater(result["parsed_note_count"], 0)
                        self.assertEqual(
                            current["declared_touches"], result["declared_touches"]
                        )
                        self.assertIn("knowledge_attachment_touches", result)
                        self.assertIn("knowledge_edges", result)

    def test_hv10_infers_scope_only_when_no_explicit_scope_exists(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("declared-scope", Path(temporary))
            (root / ".ai-docs" / "project-dna.md").write_text(
                "# Project DNA\n\n## Authentication Boundary\n\n"
                "The implementation at `src/auth.rs` needs focused review.\n",
                encoding="utf-8",
                newline="\n",
            )
            current = run_ruleset("current", root)
            inferred = run_ruleset("hv10", root)
            self.assertEqual([], current["knowledge_edges"])
            self.assertEqual(["T1"], [edge["task_id"] for edge in inferred["knowledge_edges"]])
            self.assertIn("inferred-scope:exact", inferred["knowledge_edges"][0]["rationale"])

    def test_hv11_harvests_line_ranged_contract_path_per_intent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("declared-scope", Path(temporary))
            (root / "plan.md").write_text(
                "# Contract path\n\n## Tasks\n\n"
                "- [ ] T1: Review authentication (inputs: src/auth.rs:79-81, src/missing.rs)\n"
                "- [ ] T2: Separate task\n",
                encoding="utf-8",
                newline="\n",
            )
            result = run_ruleset("hv11", root)
            self.assertEqual(
                ["src", "src/auth.rs"],
                result["knowledge_attachment_touches"]["T1"],
            )
            self.assertEqual(["T1"], [edge["task_id"] for edge in result["knowledge_edges"]])
            self.assertIn("contract-path", result["knowledge_edges"][0]["rationale"])

    def test_hv11_keeps_ambiguous_basename_off(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("declared-scope", Path(temporary))
            artifact_path = root / "codegraph.json"
            artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
            artifact["nodes"]["other-auth"] = {"path": "other/auth.rs"}
            artifact_path.write_text(json.dumps(artifact), encoding="utf-8", newline="\n")
            (root / "plan.md").write_text(
                "# Ambiguous\n\n## Tasks\n\n- [ ] T1: Review (inputs: auth.rs)\n",
                encoding="utf-8",
                newline="\n",
            )
            result = run_ruleset("hv11", root)
            self.assertNotIn("T1", result["knowledge_attachment_touches"])
            self.assertEqual([], result["knowledge_edges"])
            self.assertTrue(
                any("ambiguous" in omission["detail"] for omission in result["omissions"])
            )

    def test_hv11_uses_file_inventory_and_parent_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("codegraph-unavailable", Path(temporary))
            (root / "plan.md").write_text(
                "# New file\n\n## Tasks\n\n"
                "- [ ] T1: Add service helper (inputs: src/new/helper.rs)\n",
                encoding="utf-8",
                newline="\n",
            )
            result = run_ruleset("hv11", root)
            self.assertEqual(["src"], result["knowledge_attachment_touches"]["T1"])
            self.assertEqual(["T1"], [edge["task_id"] for edge in result["knowledge_edges"]])
            self.assertIn("fallback:parent-directory", result["knowledge_edges"][0]["rationale"])

    def test_hv12_bm25_shortlist_then_match_strength(self):
        shortlist = bm25_shortlist(
            "authentication boundary",
            [("unrelated", "queue storage"), ("auth", "authentication boundary review")],
        )
        self.assertEqual("auth", shortlist[0][0])
        self.assertGreater(
            _match_strength(["src/auth.rs"], ["src/auth.rs"]),
            _match_strength(["auth.rs"], ["src/auth.rs"]),
        )
        self.assertGreater(
            _match_strength(["auth.rs"], ["src/auth.rs"]),
            _match_strength(["src/other.rs"], ["src/auth.rs"]),
        )


if __name__ == "__main__":
    unittest.main()
