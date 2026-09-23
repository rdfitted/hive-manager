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
            self.assertEqual({"pass", "fail"}, {row["label"]["result"] for row in human_rows})

    def test_qa_inputs_are_normalized_to_criterion_result_shape(self):
        cases = (
            ("Pass", {"result": "pass"}),
            ("pass", {"result": "pass"}),
            ("FAIL", {"result": "fail"}),
            ("blocked", {"result": "blocked"}),
            ("scored 7.5", {"result": {"scored": 7.5}}),
            ('{"measured": 42}', {"result": {"measured": 42}}),
            ('{"result": {"scored": 8}}', {"result": {"scored": 8}}),
        )
        for response, expected in cases:
            with self.subTest(response=response), tempfile.TemporaryDirectory() as temporary:
                ledger_path = Path(temporary) / "ledger.jsonl"
                add_source(ledger_path, "source-a", "evidence")
                self.assertEqual(1, label.label_session(
                    ledger_path, input_fn=lambda prompt: response, output=io.StringIO(),
                ))
                rows = [row for row in ledger.read_records([ledger_path])
                        if row.get("source") == "human-label"]
                self.assertEqual([expected], [row["label"] for row in rows])

    def test_invalid_input_reprompts_without_writing_a_row(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "evidence")
            prompts = []

            def answer(prompt):
                prompts.append(prompt)
                if len(prompts) == 1:
                    return "garbage"
                self.assertEqual(1, len(list(ledger.read_records([ledger_path]))))
                return "Pass"

            output = io.StringIO()
            self.assertEqual(1, label.label_session(
                ledger_path, input_fn=answer, output=output,
            ))
            self.assertEqual(2, len(prompts))
            self.assertIn("Invalid label", output.getvalue())
            rows = [row for row in ledger.read_records([ledger_path])
                    if row.get("source") == "human-label"]
            self.assertEqual([{"result": "pass"}], [row["label"] for row in rows])

    def test_invalid_json_criterion_result_can_be_skipped_without_a_row(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "evidence")
            answers = iter(('true', '{"scored": false}', "skip"))
            self.assertEqual(0, label.label_session(
                ledger_path, input_fn=lambda prompt: next(answers), output=io.StringIO(),
            ))
            self.assertEqual(1, len(list(ledger.read_records([ledger_path]))))

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
                ledger_path, split="heldout", input_fn=lambda prompt: "pass",
                output=io.StringIO(),
            ))
            human_rows = [
                row for row in ledger.read_records([ledger_path])
                if row.get("kind") == "outcome" and row.get("source") == "human-label"
            ]
            self.assertEqual(["source-heldout"], [row["decision_id"] for row in human_rows])
            self.assertEqual({"result": "pass"}, human_rows[0]["label"])

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
