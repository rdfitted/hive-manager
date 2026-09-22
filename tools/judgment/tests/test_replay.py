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
            arguments = [
                "--sessions",
                str(sessions),
                "--ledger",
                str(ledger_path),
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
            self.assertEqual(0, report["production_attached"])
            self.assertEqual("upper bound (current knowledge copy)", report["path_miss_label"])
            repository = report["repositories"][repo.name]
            self.assertEqual(3, repository["sessions_seen"])
            self.assertEqual(1, repository["parseable_sessions"])
            self.assertEqual(1, repository["pre_grammar_sessions"])
            self.assertEqual(1, repository["unparseable_sessions"])
            named = {row["session_id"]: row["reason"] for row in report["named_sessions"]}
            self.assertEqual(
                "no planning retrieval possible",
                named[SESSION_IDS["pre-grammar"]],
            )
            self.assertIn("unterminated", named[SESSION_IDS["unparseable"]])
            for ruleset in retrieval_replay.RULESET_ORDER:
                score = repository["scorecards"][ruleset]
                self.assertEqual(0, score["production_attached"])
                self.assertIn("touch_coverage", score)
                self.assertIn("zero_knowledge_tasks", score)
                self.assertIn("attachable_share", score)
                self.assertIn("path_unscoped_pairs", score)
                self.assertEqual("upper bound (current knowledge copy)", score["path_miss_label"])
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
                )
            with self.assertRaisesRegex(ValueError, "judgment ledger"):
                retrieval_replay.run_replay(
                    [sessions],
                    ledger_path=repo / "ledger.jsonl",
                    stale_report=outside,
                )
            self.assertFalse((repo / "stale.json").exists())
            self.assertFalse((repo / "ledger.jsonl").exists())


class ReplayPlumbingTests(unittest.TestCase):
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
