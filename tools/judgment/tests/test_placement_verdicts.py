"""Strict coverage gate for R2's machine-readable D1-D8 verdicts."""

import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path(__file__).parent / "fixtures" / "placement-verdicts.json"
TABLES = ROOT / "decision-tables.json"
DECISION_IDS = {f"D{number}" for number in range(1, 9)}
VERDICTS = {"invoke", "shadow-log-only", "decline"}
REASON_CLASSES = {"measured-number", "named-gate", "not-yet-measurable"}
GATES = {"client-data exposure", "latency budget", "determinism requirement"}


def _decisions() -> list[dict]:
    fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
    if fixture.get("version") != 1 or not isinstance(fixture.get("decisions"), list):
        raise AssertionError("invalid placement fixture")
    return fixture["decisions"]


class PlacementVerdictTests(unittest.TestCase):
    def test_every_point_has_exactly_one_verdict_and_reason(self):
        decisions = _decisions()
        ids = [row["id"] for row in decisions]
        self.assertEqual(DECISION_IDS, set(ids))
        self.assertEqual(len(ids), len(set(ids)), "duplicate decision point verdict")
        for row in decisions:
            with self.subTest(point=row["id"]):
                self.assertIn(row.get("verdict"), VERDICTS)
                self.assertIn(row.get("reason_class"), REASON_CLASSES)
                if row["verdict"] == "invoke":
                    self.assertIn(row.get("judge_class"),
                                  {"deterministic rule", "proxy LLM", "Jev"})
                else:
                    self.assertIsNone(row.get("judge_class"))
                if row["reason_class"] == "measured-number":
                    self.assertGreater(row.get("n", 0), 0)
                    self.assertIs(type(row.get("usable")), bool)
                elif row["reason_class"] == "named-gate":
                    self.assertIn(row.get("gate"), GATES)
                else:
                    self.assertTrue(row.get("missing_data"))
                    self.assertTrue(row.get("flip_condition"))
                if row.get("surface") is None:
                    self.assertIsNone(row.get("surface_state"))
                else:
                    self.assertIn(row.get("surface_state"), {"live", "proposed"})

    def test_every_named_surface_has_a_decision_table_entry(self):
        surfaces = json.loads(TABLES.read_text(encoding="utf-8"))["surfaces"]
        named = {row["surface"] for row in _decisions() if row.get("surface")}
        self.assertEqual({"hive.retrieval.spawn", "hive.retrieval.delivery"}, named)
        for surface in named:
            with self.subTest(surface=surface):
                self.assertIn(surface, surfaces, "J must integrate the T15 table handoff")
                self.assertEqual("shadow", surfaces[surface]["mode"])
                self.assertEqual("multiclass", surfaces[surface]["question_type"])


if __name__ == "__main__":
    unittest.main()
