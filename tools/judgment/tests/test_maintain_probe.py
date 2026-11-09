"""Tests for the code.maintain probe's scoring. No network: answers are canned."""
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import maintain_probe  # noqa: E402


def answers(single: float, naming: float, local: float, many: float, covers: float) -> dict:
    return {"single_job": {"type": "score", "score": single}, "naming": {"type": "score", "score": naming},
            "local_reasoning": {"type": "score", "score": local},
            "more_than_one_job": {"type": "noul", "noul": many},
            "name_covers_behavior": {"type": "noul", "noul": covers}}


class Scoring(unittest.TestCase):
    def test_auc_is_one_when_every_good_outscores_every_poor(self):
        self.assertEqual(maintain_probe.auc([0.9, 0.8], [0.1, 0.2]), 1.0)

    def test_auc_is_half_without_separation_and_none_without_both_groups(self):
        self.assertEqual(maintain_probe.auc([0.5], [0.5]), 0.5)
        self.assertIsNone(maintain_probe.auc([0.5], []))

    def test_score_answers_are_scaled_by_the_top_level(self):
        self.assertEqual(maintain_probe.value_of({"type": "score", "score": 3.0}, 4), 1.0)
        self.assertEqual(maintain_probe.value_of({"type": "noul", "noul": 0.25}, None), 0.25)

    def test_summary_separates_labels_and_inverts_the_negative_question(self):
        results = {"items": {
            "clean": {"group": "g", "labels": {"single_job": "good", "naming": "good", "local_reasoning": "good"},
                      "answers": [answers(3, 3, 3, 0.1, 0.9)] * 2},
            "messy": {"group": "p", "labels": {"single_job": "poor", "naming": "poor", "local_reasoning": "poor"},
                      "answers": [answers(0, 1, 0, 0.9, 0.1), answers(0, 1, 0, 0.8, 0.2)]}}}
        summary = maintain_probe.summarize(results)
        for question in summary.values():
            self.assertEqual(question["auc"], 1.0, question)
        # "more than one job" is a negative question: a high answer must count against the function.
        many = summary["more_than_one_job"]
        self.assertGreater(many["mean_good"], many["mean_poor"])
        self.assertAlmostEqual(many["max_run_spread"], 0.1)
        self.assertEqual(summary["single_job"]["decision_flips"], 0)

    def test_unlabelled_dimensions_are_left_out_of_the_separation(self):
        results = {"items": {"x": {"group": "g", "labels": {"single_job": "good"},
                                   "answers": [answers(3, 0, 0, 0.0, 0.0)]}}}
        summary = maintain_probe.summarize(results)
        self.assertEqual((summary["naming"]["labelled_good"], summary["naming"]["labelled_poor"]), (0, 0))
        self.assertIsNone(summary["naming"]["auc"])


class Fixtures(unittest.TestCase):
    def test_every_fixture_is_labelled_and_every_question_is_well_formed(self):
        fixtures = json.loads(maintain_probe.FIXTURES_PATH.read_text(encoding="utf-8"))
        spec = json.loads(maintain_probe.SPEC_PATH.read_text(encoding="utf-8"))
        self.assertGreaterEqual(len(fixtures["functions"]), 20)
        for item in fixtures["functions"]:
            self.assertTrue(item["code"].strip() and item["labels"], item["id"])
            self.assertLessEqual(set(item["labels"].values()), {"good", "poor"}, item["id"])
        self.assertNotEqual(spec["model"], "jev-latest")  # a probe result is only comparable on a pinned version
        for qid, question in spec["questions"].items():
            if question["type"] == "score":
                self.assertGreaterEqual(len(question["criteria"]), 2, qid)
            else:
                self.assertEqual(set(question["criteria"]), {"true", "false"}, qid)


if __name__ == "__main__":
    unittest.main()
