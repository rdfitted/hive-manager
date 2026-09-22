import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import uuid
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import ledger  # noqa: E402
import retrieval_replay  # noqa: E402
import retrieval_rules  # noqa: E402


SESSION_IDS = {
    "parseable": "11111111-1111-4111-8111-111111111111",
    "pre-grammar": "22222222-2222-4222-8222-222222222222",
    "unparseable": "33333333-3333-4333-8333-333333333333",
}


def _real_ledger_paths() -> tuple[Path, Path]:
    appdata = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
    return (
        appdata / "hive-manager" / "judgments" / "ledger.jsonl",
        Path.home() / ".claude" / "hooks" / "logs" / "judgments" / "ledger.jsonl",
    )


def _ledger_state(path: Path) -> tuple[bool, int | None]:
    return path.exists(), path.stat().st_size if path.exists() else None


def _tree_hashes(root: Path) -> dict[str, str]:
    return {
        path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8", newline="\n")


def _make_fake_repo(base: Path) -> tuple[Path, Path]:
    repo = base / "synthetic-repo"
    sessions = repo / ".hive-manager"
    _write(
        repo / ".ai-docs" / "project-dna.md",
        "# Project DNA\n\n## Authentication Boundary\n\n"
        "- **Scope**: `src/auth.rs`\n- Keep authentication changes explicit.\n\n"
        "## Authentication Advisory\n\n"
        "Keep authentication reviews focused and explicit.\n",
    )
    _write(repo / ".ai-docs" / "bug-patterns.md", "# Bug Patterns\n")
    _write(
        repo / ".ai-docs" / "learnings.jsonl",
        '{"date":"2000-01-01","insight":"synthetic prior learning"}\n'
        '{"date":"2999-01-01","insight":"synthetic future learning"}\n',
    )
    _write(repo / ".ai-docs" / "curation-state.json", '{"last_curated_line":2}\n')

    parseable = sessions / SESSION_IDS["parseable"]
    _write(
        parseable / "plan.md",
        "# Synthetic replay\n\n## Tasks\n\n"
        "- [ ] [backend] T1: Review authentication (inputs: file:src/auth.rs)\n",
    )
    _write(
        parseable / "artifacts" / "codegraph.json",
        json.dumps(
            {
                "root": "<ROOT>",
                "language": "rust",
                "nodes": {"auth": {"path": "src/auth.rs"}},
            }
        )
        + "\n",
    )
    _write(
        sessions / SESSION_IDS["pre-grammar"] / "plan.md",
        "# Legacy plan\n\n- Review authentication without stable task grammar.\n",
    )
    _write(
        sessions / SESSION_IDS["unparseable"] / "plan.md",
        "# Broken plan\n\n## Tasks\n\n"
        "- [ ] T1: Review authentication (inputs: file:src/auth.rs\n",
    )
    return repo, sessions


class ReplayEndToEndTests(unittest.TestCase):
    def test_replay_columns_use_explicit_entry_modes(self):
        expected_modes = {
            "current": "compose",
            "hv10": "plan-ready",
            "hv11": "plan-ready",
            "hv10+hv11": "plan-ready",
            "hv12": "plan-ready",
            "production_attached": "observed",
        }
        self.assertEqual(expected_modes, retrieval_replay.COLUMN_ENTRY_MODES)

        observed = []

        def record_entry(ruleset: str, root: Path) -> dict:
            entry = (root / "entry.txt").read_text(encoding="utf-8").strip()
            observed.append((ruleset, entry))
            return {"ruleset": ruleset, "entry": entry}

        with tempfile.TemporaryDirectory() as temporary:
            repo, sessions = _make_fake_repo(Path(temporary))
            session = sessions / SESSION_IDS["parseable"]
            with patch.object(
                retrieval_replay, "run_ruleset", side_effect=record_entry
            ):
                matrix, comparisons = retrieval_replay.evaluate_replay_session(
                    repo, session, ["src/auth.rs"]
                )

        self.assertEqual(
            [
                ("current", "compose"),
                ("hv10", "compose"),
                ("hv11", "compose"),
                ("hv10+hv11", "compose"),
                ("hv12", "compose"),
                ("hv10", "plan-ready"),
                ("hv11", "plan-ready"),
                ("hv10+hv11", "plan-ready"),
                ("hv12", "plan-ready"),
            ],
            observed,
        )
        self.assertEqual("compose", matrix["current"]["entry"])
        for ruleset in retrieval_replay.RULESET_ORDER[1:]:
            self.assertEqual("plan-ready", matrix[ruleset]["entry"])
            self.assertEqual("compose", comparisons[ruleset][0]["entry"])
            self.assertEqual("plan-ready", comparisons[ruleset][1]["entry"])

    def test_synthetic_cli_is_read_only_complete_and_idempotent(self):
        real_paths = _real_ledger_paths()
        real_before = {path: _ledger_state(path) for path in real_paths}
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            repo, sessions = _make_fake_repo(base)
            ai_docs = repo / ".ai-docs"
            hashes_before = _tree_hashes(ai_docs)
            ledger_path = base / "output" / "ledger.jsonl"
            stale_report = base / "output" / "stale-scopes.json"
            session_store = base / "session-store"
            _write(
                session_store
                / SESSION_IDS["parseable"]
                / "state"
                / "work-graph.json",
                json.dumps(
                    {
                        "nodes": [],
                        "edges": [
                            {
                                "source": "context::knowledge::one",
                                "target": "T1",
                                "kind": "informs",
                                "provenance": "knowledge",
                            },
                            {
                                "source": "context::knowledge::two",
                                "target": "T1",
                                "kind": "informs",
                                "provenance": "knowledge",
                            },
                            {
                                "source": "T1",
                                "target": "observation::one",
                                "kind": "informs",
                                "provenance": "runtime",
                            },
                        ],
                        "omissions": [],
                    }
                )
                + "\n",
            )
            arguments = [
                "--sessions",
                str(sessions),
                "--ledger",
                str(ledger_path),
                "--session-store",
                str(session_store),
                "--stale-report",
                str(stale_report),
            ]
            with patch.object(
                retrieval_replay,
                "tracked_files",
                return_value=(["src/auth.rs"], None),
            ), patch.object(
                retrieval_replay,
                "recover_changed_files",
                return_value=({"src/auth.rs"}, "recovered from local ref"),
            ):
                first_stdout = io.StringIO()
                with redirect_stdout(first_stdout):
                    self.assertEqual(0, retrieval_replay.main(arguments))
                first_bytes = ledger_path.read_bytes()
                second_stdout = io.StringIO()
                with redirect_stdout(second_stdout):
                    self.assertEqual(0, retrieval_replay.main(arguments))

            self.assertEqual(first_bytes, ledger_path.read_bytes())
            self.assertEqual(hashes_before, _tree_hashes(ai_docs))
            self.assertTrue(stale_report.is_file())
            self.assertFalse(retrieval_replay._is_within(stale_report, repo))
            self.assertEqual((len(first_bytes.splitlines()), []), ledger.validate_file([ledger_path]))

            report = json.loads(first_stdout.getvalue())
            self.assertEqual(report, json.loads(second_stdout.getvalue()))
            self.assertEqual(2, report["production_attached"])
            self.assertEqual(
                {
                    "current": "compose",
                    "hv10": "plan-ready",
                    "hv11": "plan-ready",
                    "hv10+hv11": "plan-ready",
                    "hv12": "plan-ready",
                    "production_attached": "observed",
                },
                report["entry_modes"],
            )
            self.assertEqual("upper bound (current knowledge copy)", report["path_miss_label"])
            repository = report["repositories"][repo.name]
            self.assertEqual(3, repository["sessions_seen"])
            self.assertEqual(1, repository["parseable_sessions"])
            self.assertEqual(1, repository["pre_grammar_sessions"])
            self.assertEqual(1, repository["unparseable_sessions"])
            self.assertEqual(2, repository["production_attached"])
            named = {row["session_id"]: row["reason"] for row in report["named_sessions"]}
            self.assertEqual(
                "no planning retrieval possible",
                named[SESSION_IDS["pre-grammar"]],
            )
            self.assertIn("unterminated", named[SESSION_IDS["unparseable"]])
            for ruleset in retrieval_replay.RULESET_ORDER:
                score = repository["scorecards"][ruleset]
                self.assertEqual(2, score["production_attached"])
                self.assertIn("touch_coverage", score)
                self.assertIn("zero_knowledge_tasks", score)
                self.assertIn("attachable_share", score)
                self.assertIn("path_unscoped_pairs", score)
                self.assertEqual("upper bound (current knowledge copy)", score["path_miss_label"])
            for ruleset in retrieval_replay.RULESET_ORDER[1:]:
                comparison = report["entry_mode_comparison"][ruleset]
                self.assertIn("compose_knowledge_pairs", comparison)
                self.assertIn("plan_ready_knowledge_pairs", comparison)
                self.assertIsInstance(comparison["different"], bool)
            self.assertGreater(
                repository["scorecards"]["hv12"]["path_unscoped_pairs"], 0
            )

            rows = [json.loads(line) for line in first_bytes.decode("utf-8").splitlines()]
            decisions = [row for row in rows if row["kind"] == "decision"]
            outcomes = [row for row in rows if row["kind"] == "outcome"]
            measurable_pairs = sum(
                score["path_relevant_pairs"] + score["path_miss_pairs"]
                for score in repository["scorecards"].values()
            )
            self.assertEqual(measurable_pairs, len(outcomes))
            self.assertGreater(len(decisions), 0)
            self.assertEqual(len(decisions), len({row["decision_id"] for row in decisions}))
            for row in decisions:
                self.assertEqual(5, uuid.UUID(row["decision_id"]).version)
                self.assertIn(
                    row["surface"],
                    {"hive.retrieval.attach.note", "hive.retrieval.attach.task"},
                )
                self.assertEqual("code", row["judge"])
                self.assertEqual("shadow", row["mode"])
        real_after = {path: _ledger_state(path) for path in real_paths}
        self.assertEqual(real_before, real_after)

    def test_output_paths_must_be_outside_replayed_repository(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            repo, sessions = _make_fake_repo(base)
            outside = base / "outside.json"
            with self.assertRaisesRegex(ValueError, "stale-scope report"):
                retrieval_replay.run_replay(
                    [sessions],
                    ledger_path=outside,
                    stale_report=repo / "stale.json",
                    session_store_root=base / "session-store",
                )
            with self.assertRaisesRegex(ValueError, "judgment ledger"):
                retrieval_replay.run_replay(
                    [sessions],
                    ledger_path=repo / "ledger.jsonl",
                    stale_report=outside,
                    session_store_root=base / "session-store",
                )
            self.assertFalse((repo / "stale.json").exists())
            self.assertFalse((repo / "ledger.jsonl").exists())


class ReplayPlumbingTests(unittest.TestCase):
    def test_replay_names_parseable_session_without_a_persisted_work_graph(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            repo, sessions = _make_fake_repo(base)
            result = {
                "declared_touches": {},
                "knowledge_attachment_touches": {},
                "knowledge_edges": [],
                "context_nodes": [],
                "omissions": [],
                "hub_lints": [],
            }
            matrix = {
                ruleset: result for ruleset in retrieval_replay.RULESET_ORDER
            }
            comparisons = {
                ruleset: (result, result)
                for ruleset in retrieval_replay.RULESET_ORDER[1:]
            }

            with patch.object(
                retrieval_replay,
                "tracked_files",
                return_value=(["src/auth.rs"], None),
            ), patch.object(
                retrieval_replay,
                "recover_changed_files",
                return_value=({"src/auth.rs"}, "recovered from local ref"),
            ), patch.object(
                retrieval_replay,
                "evaluate_replay_session",
                return_value=(matrix, comparisons),
            ):
                report = retrieval_replay.run_replay(
                    [sessions],
                    ledger_path=base / "output" / "ledger.jsonl",
                    stale_report=base / "output" / "stale-scopes.json",
                    session_store_root=base / "session-store",
                )

        self.assertIn(
            {
                "repo": repo.name,
                "session_id": SESSION_IDS["parseable"],
                "reason": "production work graph missing",
            },
            report["named_sessions"],
        )
        self.assertEqual(0, report["production_attached"])

    def test_replay_reports_missing_edge_context_clearly(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            repo, sessions = _make_fake_repo(base)
            result = {
                "declared_touches": {},
                "knowledge_attachment_touches": {},
                "knowledge_edges": [
                    {
                        "task_id": "T1",
                        "context_node_id": "context::knowledge::missing",
                        "rationale": "synthetic invalid edge",
                    }
                ],
                "context_nodes": [],
                "omissions": [],
                "hub_lints": [],
            }
            matrix = {
                ruleset: result for ruleset in retrieval_replay.RULESET_ORDER
            }
            comparisons = {
                ruleset: (result, result)
                for ruleset in retrieval_replay.RULESET_ORDER[1:]
            }

            with patch.object(
                retrieval_replay,
                "tracked_files",
                return_value=(["src/auth.rs"], None),
            ), patch.object(
                retrieval_replay,
                "recover_changed_files",
                return_value=({"src/auth.rs"}, "recovered from local ref"),
            ), patch.object(
                retrieval_replay,
                "evaluate_replay_session",
                return_value=(matrix, comparisons),
            ):
                with self.assertRaisesRegex(
                    AssertionError,
                    "current.*missing context node.*context::knowledge::missing",
                ):
                    retrieval_replay.run_replay(
                        [sessions],
                        ledger_path=base / "output" / "ledger.jsonl",
                        stale_report=base / "output" / "stale-scopes.json",
                        session_store_root=base / "session-store",
                    )

    def test_task_status_matches_exact_task_token(self):
        result = {
            "declared_touches": {},
            "knowledge_attachment_touches": {
                "T1": ["src/one.rs"],
                "T12": ["src/twelve.rs"],
            },
            "_task_resolution_failures": {
                "T12": [retrieval_rules.TASK_PATH_UNRESOLVED_DETAIL],
            },
            "omissions": [
                {
                    "examples": ["T12: missing.rs"],
                }
            ],
        }

        self.assertEqual(
            "contract-path", retrieval_replay._task_status(result, "T1")
        )
        self.assertEqual("partial", retrieval_replay._task_status(result, "T12"))
        result["_task_resolution_failures"] = {
            "T1": [retrieval_rules.TASK_PATH_UNRESOLVED_DETAIL]
        }
        self.assertEqual("partial", retrieval_replay._task_status(result, "T1"))

    def test_task_status_uses_uncapped_failure_map_and_ignores_undeclared(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "project"
            ai_docs = root / ".ai-docs"
            tasks = "".join(
                f"- [ ] T{index}: Partial harvested task "
                "(inputs: src/auth.rs, missing.rs)\n"
                for index in range(1, 7)
            )
            tasks += (
                "- [ ] T7: Fully resolved harvested task "
                "(inputs: src/auth.rs)\n"
            )
            _write(root / "plan.md", f"# Status fixture\n\n## Tasks\n\n{tasks}")
            _write(root / "entry.txt", "plan-ready\n")
            _write(root / "files.txt", "src/auth.rs\n")
            _write(root / "none", "Codegraph intentionally unavailable.\n")
            _write(
                ai_docs / "project-dna.md",
                "# Project DNA\n\n## Authentication\n\n"
                "- **Scope**: `src/auth.rs`\n"
                "- Keep authentication work attached.\n",
            )
            _write(ai_docs / "bug-patterns.md", "# Bug Patterns\n\n## Bugs\n")
            _write(ai_docs / "curation-state.json", '{"last_curated_line":0}\n')
            _write(ai_docs / "learnings.jsonl", "")

            result = retrieval_rules.run_ruleset("hv10+hv11", root)

        undeclared = next(
            omission
            for omission in result["omissions"]
            if omission["detail"]
            == "explicit task touch intent was not declared"
        )
        self.assertEqual(7, undeclared["count"])
        self.assertEqual(5, len(undeclared["examples"]))
        for index in range(1, 7):
            self.assertEqual(
                "partial", retrieval_replay._task_status(result, f"T{index}")
            )
        self.assertEqual(
            "contract-path", retrieval_replay._task_status(result, "T7")
        )

    def test_note_reason_ignores_empty_touch_lists(self):
        result = {
            "knowledge_attachment_touches": {"T1": []},
            "hub_lints": [],
            "omissions": [{"reason": "codegraph_unavailable"}],
        }
        context = {"id": "context::knowledge::one", "scope": ["src"]}

        self.assertEqual(
            "codegraph-unavailable",
            retrieval_replay._note_reason(result, context),
        )

    def test_touch_counts_ignore_tasks_with_empty_path_lists(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            repo, sessions = _make_fake_repo(base)
            _write(
                sessions / SESSION_IDS["parseable"] / "plan.md",
                "# Synthetic replay\n\n## Tasks\n\n"
                "- [ ] [backend] T1: Empty touch set\n"
                "- [ ] [backend] T2: Resolved touch set\n",
            )
            result = {
                "declared_touches": {},
                "knowledge_attachment_touches": {
                    "T1": [],
                    "T2": ["src/auth.rs"],
                },
                "knowledge_edges": [],
                "context_nodes": [],
                "omissions": [],
                "hub_lints": [],
            }
            matrix = {
                ruleset: result for ruleset in retrieval_replay.RULESET_ORDER
            }
            comparisons = {
                ruleset: (result, result)
                for ruleset in retrieval_replay.RULESET_ORDER[1:]
            }
            ledger_path = base / "output" / "ledger.jsonl"
            stale_report = base / "output" / "stale-scopes.json"

            with patch.object(
                retrieval_replay,
                "tracked_files",
                return_value=(["src/auth.rs"], None),
            ), patch.object(
                retrieval_replay,
                "recover_changed_files",
                return_value=({"src/auth.rs"}, "recovered from local ref"),
            ), patch.object(
                retrieval_replay,
                "evaluate_replay_session",
                return_value=(matrix, comparisons),
            ):
                report = retrieval_replay.run_replay(
                    [sessions],
                    ledger_path=ledger_path,
                    stale_report=stale_report,
                    session_store_root=base / "session-store",
                )

        scorecards = report["repositories"][repo.name]["scorecards"]
        for score in scorecards.values():
            self.assertEqual(1, score["tasks_with_touches"])
        for comparison in report["entry_mode_comparison"].values():
            self.assertEqual(1, comparison["compose_tasks_with_touches"])
            self.assertEqual(1, comparison["plan_ready_tasks_with_touches"])

    def test_stale_fixture_populates_stale_scope_report_separately(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            repo, sessions = _make_fake_repo(base)
            _write(
                repo / ".ai-docs" / "project-dna.md",
                (repo / ".ai-docs" / "project-dna.md").read_text(encoding="utf-8")
                + "\n## Removed module advisory\n\n"
                + "The former `src/removed.rs` path should not be reused.\n",
            )
            _write(
                sessions / SESSION_IDS["parseable"] / "plan.md",
                "# Synthetic replay\n\n## Tasks\n\n"
                "- [ ] T1: Review removed module "
                "(inputs: src/auth.rs, missing.rs)\n",
            )
            ledger_path = base / "output" / "ledger.jsonl"
            stale_report = base / "output" / "stale-scopes.json"

            with patch.object(
                retrieval_replay,
                "tracked_files",
                return_value=(["src/auth.rs"], None),
            ), patch.object(
                retrieval_replay,
                "recover_changed_files",
                return_value=({"src/auth.rs"}, "recovered from local ref"),
            ):
                retrieval_replay.run_replay(
                    [sessions],
                    ledger_path=ledger_path,
                    stale_report=stale_report,
                    session_store_root=base / "session-store",
                )

            stale = json.loads(stale_report.read_text(encoding="utf-8"))

        stale_scopes = stale["stale_scope_omissions"]
        unresolved_tasks = stale["unresolved_task_path_omissions"]
        self.assertGreater(len(stale_scopes), 0)
        self.assertGreater(len(unresolved_tasks), 0)
        self.assertEqual(
            {retrieval_rules.INFERRED_SCOPE_STALE_DETAIL},
            {row["detail"] for row in stale_scopes},
        )
        self.assertEqual(
            {retrieval_rules.TASK_PATH_UNRESOLVED_DETAIL},
            {row["detail"] for row in unresolved_tasks},
        )

    def test_ledger_resolution_precedence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            explicit = base / "explicit.jsonl"
            environment = base / "environment.jsonl"
            appdata = base / "appdata"
            with patch.dict(
                os.environ,
                {"JUDGMENT_LEDGER": str(environment), "APPDATA": str(appdata)},
                clear=False,
            ):
                self.assertEqual(explicit, retrieval_replay.resolve_ledger_path(explicit))
                self.assertEqual(environment, retrieval_replay.resolve_ledger_path())
            with patch.dict(os.environ, {"APPDATA": str(appdata)}, clear=False):
                os.environ.pop("JUDGMENT_LEDGER", None)
                self.assertEqual(
                    appdata / "hive-manager" / "judgments" / "ledger.jsonl",
                    retrieval_replay.resolve_ledger_path(),
                )

    def test_changed_files_use_only_local_refs_and_merge_base(self):
        calls: list[list[str]] = []

        def fake_git(_repo: Path, arguments: list[str]) -> subprocess.CompletedProcess[bytes]:
            calls.append(arguments)
            if arguments[:3] == ["show-ref", "--verify", "--quiet"]:
                reference = arguments[3]
                found = reference in {
                    "refs/remotes/origin/hive/session-1/primary",
                    "refs/heads/main",
                }
                return subprocess.CompletedProcess(arguments, 0 if found else 1, b"", b"")
            if arguments[0] == "rev-parse":
                return subprocess.CompletedProcess(arguments, 0, b"branch-tip\n", b"")
            if arguments[0] == "merge-base":
                return subprocess.CompletedProcess(arguments, 0, b"abc123\n", b"")
            if arguments[0] == "diff":
                return subprocess.CompletedProcess(
                    arguments,
                    0,
                    b"src/auth.rs\0src/worker.rs\0",
                    b"",
                )
            raise AssertionError(arguments)

        with patch.object(retrieval_replay, "_git", side_effect=fake_git):
            changed, reason = retrieval_replay.recover_changed_files(Path("repo"), "session-1")
        self.assertEqual({"src/auth.rs", "src/worker.rs"}, changed)
        self.assertEqual("recovered from local ref", reason)
        self.assertFalse(any("fetch" in call for call in calls))
        self.assertIn(["merge-base", "refs/heads/main", "branch-tip"], calls)
        self.assertIn(["diff", "--name-only", "-z", "abc123", "branch-tip", "--"], calls)

    def test_merged_branch_diff_is_recovered_from_local_merge_history(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary) / "repo"
            repo.mkdir()

            def git(*arguments: str) -> None:
                subprocess.run(
                    ["git", "-C", str(repo), *arguments],
                    check=True,
                    capture_output=True,
                )

            git("init", "-b", "main")
            git("config", "user.name", "Replay Test")
            git("config", "user.email", "replay@example.invalid")
            _write(repo / "README.md", "base\n")
            git("add", "README.md")
            git("commit", "-m", "base")
            git("switch", "-c", "hive/session-merged/primary")
            _write(repo / "src" / "feature.rs", "pub fn feature() {}\n")
            git("add", "src/feature.rs")
            git("commit", "-m", "session change")
            git("switch", "main")
            git("merge", "--no-ff", "--no-edit", "hive/session-merged/primary")

            changed, reason = retrieval_replay.recover_changed_files(
                repo, "session-merged"
            )
            self.assertEqual({"src/feature.rs"}, changed)
            self.assertEqual("recovered from local merged branch", reason)

    def test_path_relevance_uses_note_scope_and_unscoped_is_unmeasurable(self):
        changed = {"src/auth.rs"}
        self.assertFalse(
            retrieval_replay._path_relevance({"scope": ["docs/guide.md"]}, changed)
        )
        self.assertTrue(
            retrieval_replay._path_relevance({"scope": ["src/auth.rs"]}, changed)
        )
        self.assertTrue(retrieval_replay._path_relevance({"scope": ["*"]}, changed))
        self.assertIsNone(retrieval_replay._path_relevance({"scope": []}, changed))


if __name__ == "__main__":
    unittest.main()
