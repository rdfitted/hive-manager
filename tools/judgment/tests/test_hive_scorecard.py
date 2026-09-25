"""End-to-end scorecard checks for the Rust-pinned hive row shape."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = Path(__file__).resolve().parent / "fixtures"
sys.path.insert(0, str(ROOT))

import label  # noqa: E402
import ledger  # noqa: E402
import hive_normalize  # noqa: E402


def audit_group(path: Path) -> dict:
    completed = subprocess.run(
        [sys.executable, str(ROOT / "audit.py"), "--ledger", str(path),
         "--json", "--no-retrievals", "--no-history"],
        capture_output=True, text=True, check=True,
    )
    report = json.loads(completed.stdout)
    return next(group for group in report["judges"]
                if group["surface"] == "hive.qa.criterion"
                and group["judge"] == "incumbent-llm")


class HiveScorecardTests(unittest.TestCase):
    def test_canonical_rust_pinned_shape_scores_exact_agreement(self):
        shapes = json.loads((FIXTURES / "hive-scorecard-shapes.json").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "ledger.jsonl"
            for index, expected_label in enumerate(("pass", "pass", "pass", "fail")):
                answer = json.loads(shapes["criterion_answer_json"])
                row = ledger.record_decision(
                    "hive.qa.criterion",
                    {"session_id": "synthetic", "milestone_id": "one",
                     "criterion_number": index + 1, "iteration": shapes["criterion_iteration"]},
                    "incumbent-llm", answer,
                    state_hash="sha256:" + ("0" * 63) + format(index * 2 + 1, "x"),
                    decision_id=f"canonical-{index}", ledger=path,
                    rationale=shapes["criterion_rationale"],
                )
                self.assertIsNotNone(row)
                saved = ledger.record_outcome(
                    row["decision_id"], label._label_value(expected_label),
                    "human-label", ledger=path,
                )
                self.assertIsNotNone(saved)
            group = audit_group(path)
            self.assertEqual(4, group["joined"])
            self.assertEqual(4, group["n_heldout"])
            self.assertEqual(0.75, group["agreement_heldout"])

    def test_legacy_fixture_normalizes_into_temp_ledger_and_scores(self):
        source = FIXTURES / "hive-qa-legacy.jsonl"
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "normalized.jsonl"
            raw = audit_group(source)
            self.assertEqual(0.0, raw["agreement_heldout"])
            self.assertEqual((4, 4), hive_normalize.normalize_ledger(source, output))
            normalized = audit_group(output)
            self.assertEqual(2, normalized["joined"])
            self.assertEqual(2, normalized["n_heldout"])
            self.assertEqual(0.5, normalized["agreement_heldout"])
            self.assertEqual(source.read_text(encoding="utf-8").count("\n"), 4)

    def test_normalizer_refuses_to_replace_source_or_existing_output(self):
        source = FIXTURES / "hive-qa-legacy.jsonl"
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "normalized.jsonl"
            with self.assertRaises(ValueError):
                hive_normalize.normalize_ledger(source, source)
            output.write_text("keep", encoding="utf-8")
            with self.assertRaises(FileExistsError):
                hive_normalize.normalize_ledger(source, output)
            self.assertEqual("keep", output.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
