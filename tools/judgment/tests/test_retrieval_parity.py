"""The expected JSON is sorted-key, two-space UTF-8 with a final newline.

T21's Rust test reads the same file and compares decision_id, surface,
subject_ref, and answer for every kept-then-dropped reference.
"""

import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import audit  # noqa: E402
import ledger  # noqa: E402
import retrieval_replay  # noqa: E402


FIXTURE = Path(__file__).parent / "fixtures" / "retrieval-parity"
SESSION_ID = "55555555-5555-4555-8555-555555555555"
CONTEXT_FILES = ("sampled-context.json", "unsampled-context.json")


def _contexts() -> list[dict]:
    return [json.loads((FIXTURE / name).read_text(encoding="utf-8")) for name in CONTEXT_FILES]


def _projection(contexts: list[dict]) -> list[dict]:
    rows = []
    for context in contexts:
        for index, reference, features in retrieval_replay._spawn_reference_rows(context):
            rows.append({
                "decision_id": retrieval_replay._spawn_decision_id(
                    "synthetic-repo", SESSION_ID, context["agent_id"], index
                ),
                "surface": "hive.retrieval.spawn",
                "subject_ref": {
                    "repo": "synthetic-repo",
                    "session_id": SESSION_ID,
                    "agent_id": context["agent_id"],
                    "plan_task_id": context["plan_task_id"],
                    "reference_index": index,
                    "tag": reference.get("tag"),
                    "pointer": reference.get("pointer"),
                },
                "answer": {
                    "result": "used" if features["disposition"] == "kept" else "unused"
                },
            })
    return rows


class RetrievalParityTests(unittest.TestCase):
    def test_checked_in_rows_pin_uuid_and_canonical_shape(self):
        contexts = _contexts()
        self.assertEqual([True, False], [context["sampled"] for context in contexts])
        expected_bytes = (FIXTURE / "expected-rows.json").read_bytes()
        generated = json.dumps(_projection(contexts), sort_keys=True, indent=2) + "\n"
        self.assertEqual(expected_bytes, generated.encode("utf-8"))
        self.assertEqual(5, len(json.loads(expected_bytes)))
        self.assertEqual(
            [0, 1, 2, 0, 1],
            [row["subject_ref"]["reference_index"] for row in json.loads(expected_bytes)],
        )

    def test_live_rows_replay_without_duplicates_and_audit_agrees(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo = root / "synthetic-repo"
            session = repo / ".hive-manager" / SESSION_ID
            prompts = session / "prompts"
            prompts.mkdir(parents=True)
            (session / "plan.md").write_text(
                "# Legacy synthetic plan\n\nReview references.\n", encoding="utf-8"
            )
            for name, context in zip(CONTEXT_FILES, _contexts()):
                (prompts / f'{context["agent_id"]}-context.json').write_bytes(
                    (FIXTURE / name).read_bytes()
                )
            contexts, errors = retrieval_replay.load_spawn_contexts(session)
            self.assertEqual([], errors)
            self.assertEqual(2, len(contexts), "additive v1 fields must still load")
            ledger_path = root / "output" / "ledger.jsonl"
            ledger_path.parent.mkdir()
            expected = json.loads((FIXTURE / "expected-rows.json").read_text(encoding="utf-8"))
            fresh_path = root / "output" / "fresh.jsonl"
            known_decisions = set()
            for context in contexts:
                retrieval_replay._record_spawn_context(
                    ledger_path=fresh_path,
                    existing_decisions=known_decisions,
                    repo_name="synthetic-repo",
                    session_id=SESSION_ID,
                    context=context,
                )
            fresh_decisions = list(ledger.read_records([fresh_path]))
            self.assertEqual(
                expected,
                [{key: row[key] for key in ("decision_id", "surface", "subject_ref", "answer")}
                 for row in fresh_decisions],
            )
            self.assertTrue(all("retrieval_features" in row for row in fresh_decisions))
            for row, label in zip(expected[:2], ("used", "unused")):
                ledger.record_outcome(
                    row["decision_id"], {"result": label}, "model-ack", ledger=fresh_path
                )
            for row in expected:
                ledger.record_decision(
                    row["surface"], row["subject_ref"], "code", row["answer"],
                    question_id="knowledge_ack", question_version="1",
                    mode="shadow", decision_id=row["decision_id"], ledger=ledger_path,
                    retrieval_features={"synthetic": True},
                )
            for row, label in zip(expected[:2], ("used", "unused")):
                ledger.record_outcome(
                    row["decision_id"], {"result": label}, "model-ack",
                    ledger=ledger_path,
                )
            before = ledger_path.read_bytes()
            store = root / "store" / SESSION_ID / "state"
            store.mkdir(parents=True)
            (store / "knowledge-acks.jsonl").write_text(
                json.dumps({
                    "schema_version": "hive.knowledge-ack/v1",
                    "session_id": SESSION_ID,
                    "agent_id": contexts[0]["agent_id"],
                    "knowledge_ack": ["k1"],
                    "recorded_at": "2026-09-24T00:00:00Z",
                }) + "\n", encoding="utf-8",
            )
            with patch.object(retrieval_replay, "tracked_files", return_value=([], None)):
                retrieval_replay.run_replay(
                    [session.parent], ledger_path=ledger_path,
                    stale_report=root / "stale.json",
                    session_store_root=root / "store",
                )
            self.assertEqual(before, ledger_path.read_bytes(), "live rows must not duplicate")
            rows = list(ledger.read_records([ledger_path]))
            decisions = [row for row in rows if row["kind"] == "decision"]
            outcomes = [row for row in rows if row["kind"] == "outcome"]
            self.assertEqual(5, len(decisions))
            self.assertEqual(2, len(outcomes))
            self.assertEqual(
                expected,
                [{key: row[key] for key in ("decision_id", "surface", "subject_ref", "answer")}
                 for row in decisions],
            )
            self.assertTrue(all("retrieval_features" in row for row in decisions))
            self.assertEqual({"result": "unused"}, outcomes[1]["label"])

            tables = root / "tables.json"
            tables.write_text(json.dumps({"surfaces": {
                "hive.retrieval.spawn": {"mode": "shadow", "question_type": "multiclass"}
            }}), encoding="utf-8")
            output = io.StringIO()
            with redirect_stdout(output):
                self.assertEqual(0, audit.main([
                    "--ledger", str(fresh_path), "--tables", str(tables),
                    "--no-retrievals", "--no-history", "--json",
                ]))
            score = json.loads(output.getvalue())
            group = next(row for row in score["judges"] if row["surface"] == "hive.retrieval.spawn")
            self.assertEqual(2, group["joined"])
            correct = sum(
                (group[f"agreement_{split}"] or 0) * group[f"n_{split}"]
                for split in ("heldout", "tune")
            )
            self.assertEqual(1, correct)
            self.assertEqual(0.5, correct / group["joined"])


if __name__ == "__main__":
    unittest.main()
