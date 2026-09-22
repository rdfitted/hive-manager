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
    INFERRED_SCOPE_STALE_DETAIL,
    TASK_PATH_UNRESOLVED_DETAIL,
    TaskNode,
    _contract_intents,
    _is_path_like_token,
    _load_knowledge,
    _match_strength,
    _normalize_path_reference,
    bm25_shortlist,
    resolve_path_intent,
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
                        if current["entry"] == "compose":
                            self.assertEqual(
                                current["declared_touches"],
                                result["declared_touches"],
                            )
                        else:
                            for task_id, paths in result["declared_touches"].items():
                                self.assertTrue(paths)
                                self.assertTrue(
                                    set(paths).issubset(
                                        result["knowledge_attachment_touches"][task_id]
                                    )
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

    def test_knowledge_path_predicate_and_normalizer_match_rust(self):
        self.assertFalse(_is_path_like_token("config.max_size"))
        self.assertFalse(_is_path_like_token("settings.retry_limit"))
        self.assertTrue(_is_path_like_token("config.toml"))
        self.assertTrue(_is_path_like_token("src/pass.rs"))
        self.assertEqual("src/pass.rs", _normalize_path_reference("`src/pass.rs:10-20`"))
        self.assertEqual("src/pass.rs)", _normalize_path_reference("src/pass.rs)"))
        self.assertEqual(
            (None, None, "stale"),
            resolve_path_intent("src/pass.rs", ["src//pass.rs"]),
        )
        for value in (
            "src//pass.rs",
            "src/./pass.rs",
            "/src/pass.rs",
            "C:src/pass.rs",
        ):
            with self.subTest(value=value):
                self.assertIsNone(_normalize_path_reference(value))

    def test_knowledge_reads_fail_open_per_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("file-list-fallback", Path(temporary))
            (root / ".ai-docs" / "curation-state.json").unlink()
            (root / ".ai-docs" / "bug-patterns.md").unlink()
            (root / ".ai-docs" / "learnings.jsonl").unlink()
            gotchas, omissions, available = _load_knowledge(root)

        self.assertTrue(available)
        self.assertTrue(gotchas)
        unavailable = [
            omission
            for omission in omissions
            if omission["reason"] == "project_knowledge_unavailable"
        ]
        self.assertEqual(3, len(unavailable))
        self.assertTrue(any("curation-state.json" in item["examples"][0] for item in unavailable))
        self.assertTrue(any("bug-patterns.md" in item["examples"][0] for item in unavailable))
        self.assertTrue(any("learnings.jsonl" in item["examples"][0] for item in unavailable))

    def test_r3_fixtures_pin_unavailable_paths_and_pipeline_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            unavailable = run_ruleset(
                "hv10+hv11",
                materialize_fixture("undeclared-no-candidates", Path(temporary)),
            )
            strict = run_ruleset(
                "hv10+hv11",
                materialize_fixture("path-token-rules", Path(temporary)),
            )
            ordered = run_ruleset(
                "hv10+hv11",
                materialize_fixture("compose-omission-order", Path(temporary)),
            )

        self.assertEqual(
            [
                "codegraph output was unavailable, so repository relationships are incomplete",
                "tracked file inventory was unavailable",
                "explicit task touch intent was not declared",
            ],
            [item["detail"] for item in unavailable["omissions"]],
        )
        self.assertEqual([], unavailable["hub_lints"])
        self.assertEqual(
            {"T1": ["src/auth.rs"], "T2": ["config.toml"], "T7": ["src"]},
            strict["knowledge_attachment_touches"],
        )
        self.assertEqual(
            [
                "explicit task touch intent was not declared",
                TASK_PATH_UNRESOLVED_DETAIL,
                "one or more graph references could not be resolved",
                INFERRED_SCOPE_STALE_DETAIL,
                INFERRED_SCOPE_STALE_DETAIL,
            ],
            [item["detail"] for item in strict["omissions"]],
        )
        self.assertEqual(
            [
                "codegraph artifact was available but did not cover or resolve declared language(s): rust",
                TASK_PATH_UNRESOLVED_DETAIL,
                "one or more graph references could not be resolved",
            ],
            [item["detail"] for item in ordered["omissions"]],
        )

    def test_inferred_scope_failures_are_event_counted_per_section(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("file-list-fallback", Path(temporary))
            (root / ".ai-docs" / "project-dna.md").write_text(
                "# Project DNA\n\n"
                "## First stale scope\n\n"
                "Use `missing.rs` and retry `missing.rs`.\n\n"
                "## Second stale scope\n\n"
                "Use `missing.rs` and retry `missing.rs`.\n",
                encoding="utf-8",
                newline="\n",
            )
            result = run_ruleset("hv10", root)

        inferred = [
            omission
            for omission in result["omissions"]
            if omission["detail"] == INFERRED_SCOPE_STALE_DETAIL
        ]
        self.assertEqual(2, len(inferred))
        self.assertEqual([2, 2], [item["count"] for item in inferred])
        self.assertEqual([1, 1], [len(item["examples"]) for item in inferred])

    def test_curation_line_rejects_values_outside_rust_u64_domain(self):
        for value in ({"last_curated_line": 1 << 64}, [], {"last_curated_line": -1}):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as temporary:
                root = materialize_fixture("file-list-fallback", Path(temporary))
                (root / ".ai-docs" / "curation-state.json").write_text(
                    json.dumps(value),
                    encoding="utf-8",
                    newline="\n",
                )
                _gotchas, omissions, available = _load_knowledge(root)

                self.assertTrue(available)
                self.assertTrue(
                    any(
                        omission["reason"] == "resolution_incomplete"
                        and omission["examples"]
                        == [".ai-docs/curation-state.json:last_curated_line"]
                        for omission in omissions
                    )
                )

    def test_non_object_learning_rows_are_ignored_like_rust(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("file-list-fallback", Path(temporary))
            (root / ".ai-docs" / "curation-state.json").write_text(
                '{"last_curated_line":1}\n', encoding="utf-8", newline="\n"
            )
            (root / ".ai-docs" / "learnings.jsonl").write_text(
                '[]\n', encoding="utf-8", newline="\n"
            )
            gotchas, omissions, available = _load_knowledge(root)

        self.assertTrue(available)
        self.assertTrue(gotchas)
        self.assertFalse(any("learnings.jsonl#L1" in str(item) for item in omissions))

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
            root = materialize_fixture("file-list-fallback", Path(temporary))
            (root / "files.txt").write_text(
                "src/new/existing.rs\n", encoding="utf-8", newline="\n"
            )
            (root / "plan.md").write_text(
                "# New file\n\n## Tasks\n\n"
                "- [ ] T1: Add service helper (inputs: src/new/helper.rs)\n",
                encoding="utf-8",
                newline="\n",
            )
            result = run_ruleset("hv11", root)
            self.assertEqual(["src/new"], result["knowledge_attachment_touches"]["T1"])
            self.assertEqual([], result["knowledge_edges"])

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
            for result in (harvested, partial, fallback):
                self.assertFalse(
                    any(
                        item["reason"] == "codegraph_unavailable"
                        and item["examples"] == ["touches-resolver"]
                        for item in result["omissions"]
                    )
                )

    def test_mixed_intent_failures_match_rust_event_counts_and_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture(
                "mixed-intent-failures", Path(temporary)
            )
            result = run_ruleset("hv10+hv11", root)

        self.assertEqual(
            [
                "explicit task touch intent was not declared",
                TASK_PATH_UNRESOLVED_DETAIL,
                "knowledge path resolution was ambiguous",
            ],
            [omission["detail"] for omission in result["omissions"]],
        )
        unresolved = next(
            omission
            for omission in result["omissions"]
            if omission["detail"] == TASK_PATH_UNRESOLVED_DETAIL
        )
        self.assertEqual(3, unresolved["count"])
        self.assertEqual(
            [
                "T1: missing/declared.rs:10-20",
                "T1: missing/harvested.rs",
            ],
            unresolved["examples"],
        )
        self.assertEqual(
            {"T2": ["src/pass.rs"]}, result["declared_touches"]
        )
        self.assertEqual(
            {
                "T2": ["src/pass.rs"],
                "T3": ["src/pass.rs"],
            },
            result["knowledge_attachment_touches"],
        )
        self.assertEqual(
            {
                "T1": [TASK_PATH_UNRESOLVED_DETAIL],
                "T4": ["knowledge path resolution was ambiguous"],
            },
            result["_task_resolution_failures"],
        )

    def test_unavailable_coverage_skips_star_hub_lint(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture(
                "star-hub-unavailable", Path(temporary)
            )
            result = run_ruleset("hv10+hv11", root)

        self.assertEqual([], result["hub_lints"])
        self.assertEqual([], result["knowledge_edges"])

    def test_expanded_hub_lint_requires_changed_linkage(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("star-hub", Path(temporary))
            result = run_ruleset("hv10+hv11", root)
            self.assertEqual(1, len(result["hub_lints"]))
            self.assertEqual(
                "context applies to a high fraction of tasks; move standing guidance to the role prompt or narrow its scope",
                result["hub_lints"][0]["reason"],
            )

    def test_plan_ready_declared_base_survives_expanded_hub(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("file-list-fallback", Path(temporary))
            (root / "plan.md").write_text(
                "# Base and expanded coverage\n\n## Tasks\n\n"
                "- [ ] T1: Declared authentication change "
                "(inputs: file:src/auth.rs)\n"
                "- [ ] T2: Harvested authentication check "
                "(acceptance: src/auth.rs:10-20 remains covered)\n",
                encoding="utf-8",
                newline="\n",
            )

            result = run_ruleset("hv10+hv11", root)

        self.assertEqual({"T1": ["src/auth.rs"]}, result["declared_touches"])
        self.assertEqual(
            {"T1": ["src/auth.rs"], "T2": ["src/auth.rs"]},
            result["knowledge_attachment_touches"],
        )
        self.assertEqual(
            ["T1"],
            [edge["task_id"] for edge in result["knowledge_edges"]],
        )
        self.assertEqual(
            "knowledge attachment matched declared-scope, fallback",
            result["knowledge_edges"][0]["rationale"],
        )
        self.assertEqual(1, len(result["hub_lints"]))
        self.assertEqual(["T1", "T2"], result["hub_lints"][0]["linked_task_ids"])
        self.assertEqual(
            "expanded knowledge coverage applies to a high fraction of tasks; "
            "base attachment edges were preserved and expanded knowledge edges were withheld",
            result["hub_lints"][0]["reason"],
        )

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

    def test_plan_ready_file_inventory_validation_fails_open(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("file-list-fallback", Path(temporary))
            (root / "files.txt").write_text(
                "/absolute/path.rs\n", encoding="utf-8", newline="\n"
            )
            result = run_ruleset("hv10+hv11", root)
            self.assertTrue(
                any(
                    item["detail"] == "tracked file inventory contained an invalid path"
                    and item["examples"] == ["git ls-files"]
                    for item in result["omissions"]
                )
            )

    def test_compose_without_artifact_ignores_files_and_fails_open(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = materialize_fixture("codegraph-unavailable", Path(temporary))
            (root / "files.txt").write_text(
                "/absolute/path.rs\n", encoding="utf-8", newline="\n"
            )
            result = run_ruleset("hv10+hv11", root)
            self.assertEqual([], result["knowledge_edges"])
            self.assertTrue(
                any(
                    item["detail"] == "tracked file inventory was unavailable"
                    and item["examples"] == ["git ls-files"]
                    for item in result["omissions"]
                )
            )
            self.assertTrue(
                any(
                    item["reason"] == "codegraph_unavailable"
                    and item["examples"] == ["touches-resolver"]
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
