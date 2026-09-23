import io
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import audit  # noqa: E402
import label  # noqa: E402
import ledger  # noqa: E402


def add_source(ledger_path: Path, source_id: str, evidence: str) -> str:
    observations = {"criterion": {"number": 1}, "evidence": evidence}
    evidence_dir = ledger_path.parent / "evidence"
    evidence_dir.mkdir(exist_ok=True)
    (evidence_dir / f"{source_id}.json").write_text(
        ledger.canonical_json(observations), encoding="utf-8"
    )
    state_hash = ledger.sha256_of(observations)
    row = ledger.record_decision(
        "hive.qa.criterion", {"session_id": "synthetic"}, "incumbent-llm",
        {"result": "Fail", "rationale": "hidden from label prompt"},
        state_hash=state_hash, state_ref=f"evidence/{source_id}.json",
        decision_id=source_id, ledger=ledger_path,
    )
    assert row is not None
    return state_hash


class LabelSessionTests(unittest.TestCase):
    def test_interruption_resumes_and_never_duplicates_human_outcomes(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "first")
            add_source(ledger_path, "source-b", "second")
            answers = iter(("pass", "quit"))
            output = io.StringIO()
            self.assertEqual(1, label.label_session(
                ledger_path, input_fn=lambda prompt: next(answers), output=output,
            ))
            self.assertNotIn("hidden from label prompt", output.getvalue())
            self.assertEqual(1, label.label_session(
                ledger_path, input_fn=lambda prompt: "fail", output=io.StringIO(),
            ))
            self.assertEqual(0, label.label_session(
                ledger_path,
                input_fn=lambda prompt: self.fail("already labeled item prompted"),
                output=io.StringIO(),
            ))
            human_rows = [
                row for row in ledger.read_records([ledger_path])
                if row.get("kind") == "outcome" and row.get("source") == "human-label"
            ]
            self.assertEqual(2, len(human_rows))
            self.assertEqual({"source-a", "source-b"}, {
                row["decision_id"] for row in human_rows
            })
            self.assertEqual({"pass", "fail"}, {row["label"] for row in human_rows})

    def test_heldout_filter_uses_vendored_split(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            hashes = {}
            for index in range(20):
                evidence = f"observation-{index}"
                state_hash = ledger.sha256_of({
                    "criterion": {"number": 1}, "evidence": evidence,
                })
                split = "heldout" if audit.is_heldout(state_hash) else "tune"
                if split not in hashes:
                    hashes[split] = add_source(ledger_path, f"source-{split}", evidence)
                if len(hashes) == 2:
                    break
            self.assertEqual({"heldout", "tune"}, set(hashes))
            self.assertEqual(1, label.label_session(
                ledger_path, split="heldout", input_fn=lambda prompt: "true",
                output=io.StringIO(),
            ))
            human_rows = [
                row for row in ledger.read_records([ledger_path])
                if row.get("kind") == "outcome" and row.get("source") == "human-label"
            ]
            self.assertEqual(["source-heldout"], [row["decision_id"] for row in human_rows])
            self.assertIs(True, human_rows[0]["label"])

    def test_ten_minute_deadline_stops_before_next_prompt(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "first")
            ticks = iter((0, 601))
            self.assertEqual(0, label.label_session(
                ledger_path, minutes=10, clock=lambda: next(ticks),
                input_fn=lambda prompt: self.fail("deadline was exceeded"),
                output=io.StringIO(),
            ))


if __name__ == "__main__":
    unittest.main()
