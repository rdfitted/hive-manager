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
    TaskNode,
    _contract_intents,
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

    def test_contract_harvest_ignores_urls_and_keeps_repository_paths(self):
        node = TaskNode(
            "T1",
            "Review design",
            ["https://example.invalid/design.md and src/queue.rs:79-81"],
            [],
            [],
        )
        self.assertEqual(
            [("src/queue.rs", "contract-path")],
            _contract_intents(node),
        )

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
            self.assertIn("fallback", result["knowledge_edges"][0]["rationale"])
            self.assertIn("parent-directory", result["knowledge_edges"][0]["rationale"])

    def test_t12_inferred_fixtures_pin_match_type_provenance(self):
        cases = {
            "inferred-exact": "inferred-scope:exact",
            "inferred-basename": "inferred-scope:unique-basename",
            "inferred-suffix": "inferred-scope:path-suffix",
        }
        with tempfile.TemporaryDirectory() as temporary:
            for fixture, provenance in cases.items():
                with self.subTest(fixture=fixture):
                    root = materialize_fixture(fixture, Path(temporary))
                    result = run_ruleset("hv10", root)
                    self.assertEqual(
                        ["T1"],
                        [edge["task_id"] for edge in result["knowledge_edges"]],
                    )
                    self.assertIn(provenance, result["knowledge_edges"][0]["rationale"])
                    parameters = result["context_nodes"][0]["parameters"]
                    self.assertIn(provenance, parameters["knowledge_provenance"])

    def test_t12_inferred_ambiguous_and_stale_fixtures_record_omissions(self):
        cases = {
            "inferred-ambiguous": "inferred knowledge scope was ambiguous",
            "inferred-stale": "inferred knowledge scope found no tracked path",
        }
        with tempfile.TemporaryDirectory() as temporary:
            for fixture, detail in cases.items():
                with self.subTest(fixture=fixture):
                    root = materialize_fixture(fixture, Path(temporary))
                    result = run_ruleset("hv10", root)
                    self.assertEqual([], result["knowledge_edges"])
                    self.assertTrue(
                        any(item["detail"] == detail for item in result["omissions"])
                    )

    def test_t12_harvest_partial_and_fallback_fixtures(self):
        with tempfile.TemporaryDirectory() as temporary:
            harvested_root = materialize_fixture("harvested-line-range", Path(temporary))
            harvested = run_ruleset("hv11", harvested_root)
            self.assertEqual(
                ["src/queue.rs"],
                harvested["knowledge_attachment_touches"]["T1"],
            )
            self.assertFalse(
                any("example.invalid" in str(item) for item in harvested["omissions"])
            )

            partial_root = materialize_fixture("partial-task", Path(temporary))
            partial = run_ruleset("hv11", partial_root)
            self.assertEqual(
                ["src/auth.rs"], partial["knowledge_attachment_touches"]["T1"]
            )
            self.assertTrue(
                any(
                    item["detail"] == "knowledge path resolution found no tracked path"
                    and "T1: missing.rs" in item["examples"]
                    for item in partial["omissions"]
                )
            )

            fallback_root = materialize_fixture("file-list-fallback", Path(temporary))
            fallback = run_ruleset("hv11", fallback_root)
            self.assertEqual(
                ["src/auth.rs"], fallback["knowledge_attachment_touches"]["T1"]
            )
            self.assertIn("fallback", fallback["knowledge_edges"][0]["rationale"])

    def test_hv10_plain_files_marker_suppresses_inference_and_global_ref_inherits_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("declared-scope", Path(temporary))
            (root / ".ai-docs" / "project-dna.md").write_text(
                "# Project DNA\n\n"
                "## Explicit Files\n\n"
                "Files: `src/auth.rs`\n"
                "Mention `src/auth.rs`.\n\n"
                "## Global Mirror\n\n"
                "Use `src/auth.rs`.\n"
                "-> global: patterns/testing-strategy.md\n",
                encoding="utf-8",
                newline="\n",
            )
            result = run_ruleset("hv10", root)
            explicit = next(
                node
                for node in result["context_nodes"]
                if node["parameters"]["source_ref"] == ".ai-docs/project-dna.md#L3"
            )
            self.assertEqual([], explicit["scope"])
            self.assertNotIn("knowledge_provenance", explicit["parameters"])

            inferred_nodes = [
                node
                for node in result["context_nodes"]
                if node["parameters"]["source_ref"]
                in {".ai-docs/project-dna.md#L8", "global:patterns/testing-strategy.md"}
            ]
            self.assertEqual(2, len(inferred_nodes))
            for node in inferred_nodes:
                self.assertEqual(["src/auth.rs"], node["scope"])
                self.assertEqual(
                    "src/auth.rs=inferred-scope:exact",
                    node["parameters"]["knowledge_provenance"],
                )

    def test_file_inventory_validation_fails_open_with_stable_omission(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("codegraph-unavailable", Path(temporary))
            (root / "files.txt").write_text(
                "/absolute/path.rs\n", encoding="utf-8", newline="\n"
            )
            result = run_ruleset("hv10+hv11", root)
            self.assertEqual([], result["knowledge_edges"])
            self.assertTrue(
                any(
                    item["detail"] == "tracked file inventory contained an invalid path"
                    and item["examples"] == ["git ls-files"]
                    for item in result["omissions"]
                )
            )

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
