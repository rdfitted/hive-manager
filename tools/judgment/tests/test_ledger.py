import json
import os
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import ledger  # noqa: E402


def _real_ledger_paths() -> tuple[Path, Path]:
    appdata = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
    return (
        appdata / "hive-manager" / "judgments" / "ledger.jsonl",
        Path.home() / ".claude" / "hooks" / "logs" / "judgments" / "ledger.jsonl",
    )


def _ledger_state(path: Path) -> tuple[bool, int | None]:
    return (path.exists(), path.stat().st_size if path.exists() else None)


def _schema_errors(record: dict, schema: dict) -> list[str]:
    definition = schema["$defs"][record.get("kind", "")]
    errors = [field for field in definition["required"] if field not in record]
    for field, property_schema in definition["properties"].items():
        if field not in record:
            continue
        if "const" in property_schema and record[field] != property_schema["const"]:
            errors.append(field)
        if "enum" in property_schema and record[field] not in property_schema["enum"]:
            errors.append(field)
    return errors


class LedgerIsolationTests(unittest.TestCase):
    def test_rows_validate_against_vendored_schema(self):
        schema = json.loads(
            (JUDGMENT_ROOT / "ledger-schema.json").read_text(encoding="utf-8")
        )
        self.assertEqual("judgment-ledger/v1", schema["$id"])
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            decision = ledger.record_decision(
                "hive.retrieval.attach.note",
                {"session_id": "synthetic", "task_id": "T1"},
                "code",
                {"status": "declared"},
                mode="shadow",
                decision_id="synthetic-decision",
                ledger=ledger_path,
            )
            self.assertIsNotNone(decision)
            outcome = ledger.record_outcome(
                "synthetic-decision",
                "path-relevant",
                "downstream",
                ledger=ledger_path,
            )
            self.assertIsNotNone(outcome)
            rows = [json.loads(line) for line in ledger_path.read_text(encoding="utf-8").splitlines()]
            self.assertEqual([], [error for row in rows for error in _schema_errors(row, schema)])
            self.assertEqual((2, []), ledger.validate_file([ledger_path]))

    def test_operations_leave_both_real_ledgers_unchanged(self):
        real_paths = _real_ledger_paths()
        before = {path: _ledger_state(path) for path in real_paths}
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            row = ledger.record_decision(
                "hive.retrieval.attach.task",
                {"session_id": "synthetic", "task_id": "T1"},
                "code",
                {"status": "declared"},
                mode="shadow",
                decision_id="real-ledger-guard",
                ledger=ledger_path,
            )
            self.assertIsNotNone(row)
            self.assertTrue(ledger_path.is_file())
        after = {path: _ledger_state(path) for path in real_paths}
        self.assertEqual(before, after, "a real judgment ledger changed during tests")


if __name__ == "__main__":
    unittest.main()
