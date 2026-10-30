"""Zero-call rescoring: rules, baselines, truth sources, and the frozen bench trace."""

from __future__ import annotations

import ast
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import ledger  # noqa: E402
import rescore  # noqa: E402
import shadow_qa  # noqa: E402


BENCH_DIR = ROOT / "tests" / "fixtures" / "qa-two-sided"
BENCH = BENCH_DIR / "bench-v1.json"
TRACE = BENCH_DIR / "trace-v1.jsonl"
TABLES = json.loads((ROOT / "decision-tables.json").read_text(encoding="utf-8"))
QA_RULES = {rule["id"]: rule for rule in TABLES["rescore"]["hive.qa.criterion"]["rules"]}


def write_rows(path: Path, rows: list[dict]) -> None:
    with path.open("a", encoding="utf-8", newline="\n") as stream:
        for row in rows:
            stream.write(json.dumps(row) + "\n")


def run_rows(item: str, index: int, meets: float, misses: float, injection: float = 0.01,
             *, versions=("qv-meets", "qv-misses", "qv-injection"), state_hash: str = "sha256:0") -> list[dict]:
    """One recorded fan-out request, in the shape judge.py writes."""
    def row(surface, question, version, p, yes, no):
        return {
            "v": 1, "kind": "decision", "ts": f"2026-10-30T00:00:{index:02d}.000Z",
            "decision_id": f"{item}-{index}-{question}", "surface": surface,
            "subject_ref": {"qa_bench_item": item}, "state_hash": state_hash, "state_ref": None,
            "question_id": surface, "question_version": version, "judge": "jev",
            "model": "synthetic-model", "sampling": {}, "mode": "shadow",
            "answer": {"result": yes if p >= 0.5 else no}, "noul": p,
            "probabilities": {yes: p, no: 1 - p}, "error": None, "routed": "none",
            "source_decision_id": item, "request_id": f"{item}-{index}",
        }
    return [
        row("hive.qa.criterion", "meets", versions[0], meets, "pass", "fail"),
        row("hive.qa.criterion.misses", "misses", versions[1], misses, "fail", "pass"),
        row("hive.qa.criterion.injection", "injection", versions[2], injection, "true", "false"),
    ]


def frozen_rows() -> list[dict]:
    """Expand the compact frozen trace back into ledger rows."""
    lines = [json.loads(line) for line in TRACE.read_text(encoding="utf-8").splitlines()]
    header, runs = lines[0], lines[1:]
    hashes = {item["id"]: ledger.sha256_of(shadow_qa.observations_of(item))
              for item in json.loads(BENCH.read_text(encoding="utf-8"))["items"]}
    versions = (header["question_versions"]["hive.qa.criterion.meets"],
                header["question_versions"]["hive.qa.criterion.misses"],
                header["question_versions"]["hive.qa.criterion.injection"])
    rows = []
    for item, index, meets, misses, injection in runs:
        rows += run_rows(item, index, meets, misses, injection, versions=versions, state_hash=hashes[item])
    return rows


class RuleTests(unittest.TestCase):
    def test_combination_takes_the_smaller_of_both_answers(self):
        two_sided = QA_RULES["two_sided"]
        self.assertAlmostEqual(0.69, rescore.combined(two_sided, 0.87, 0.31))
        self.assertAlmostEqual(0.4, rescore.combined(two_sided, 0.9, 0.6))
        self.assertAlmostEqual(0.3, rescore.combined(two_sided, 0.3, 0.1))
        self.assertEqual(0.87, rescore.combined(QA_RULES["one_sided"], 0.87, 0.31))
        # Passes exactly when meets >= 0.5 and misses <= 0.5.
        self.assertEqual("pass", rescore.decide(two_sided, rescore.combined(two_sided, 0.5, 0.5)))
        self.assertEqual("fail", rescore.decide(two_sided, rescore.combined(two_sided, 0.5, 0.51)))
        self.assertEqual("fail", rescore.decide(two_sided, rescore.combined(two_sided, 0.49, 0.1)))

    def test_band_abstains_between_its_edges(self):
        band = QA_RULES["two_sided_band"]
        self.assertEqual(["pass", "abstain", "abstain", "fail"],
                         [rescore.decide(band, p) for p in (0.7, 0.69, 0.3, 0.29)])

    def test_unknown_combine_rule_is_refused(self):
        with self.assertRaisesRegex(ValueError, "unknown combine rule"):
            rescore.combined({"combine": "product"}, 0.5, 0.5)

    def test_repeatability_is_on_the_decision_with_jitter_apart(self):
        rule = QA_RULES["one_sided"]
        steady = rescore.repeatability(rule, {"a": [0.88, 0.86, 0.87]})
        self.assertEqual((1, 0, 0.02), (steady["items_repeated"], steady["decision_flips"], steady["jitter_max"]))
        flipped = rescore.repeatability(rule, {"a": [0.49, 0.51]})
        self.assertEqual(1, flipped["decision_flips"])
        self.assertEqual(0, rescore.repeatability(rule, {"a": [0.9]})["items_repeated"])

    def test_paired_sign_test_counts_only_disagreements(self):
        items = [{"truth": "fail", "probability_by_rule": {"one_sided": 0.8, "two_sided": 0.2}}] * 2
        pair = rescore.paired(QA_RULES["one_sided"], QA_RULES["two_sided"], items)
        self.assertEqual((0, 2, 0.5), (pair["a_right_b_wrong"], pair["b_right_a_wrong"], pair["sign_test_p"]))
        same = [{"truth": "pass", "probability_by_rule": {"one_sided": 0.8, "two_sided": 0.8}}]
        self.assertIsNone(rescore.paired(QA_RULES["one_sided"], QA_RULES["two_sided"], same)["sign_test_p"])


class LedgerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.ledger = self.root / "ledger.jsonl"

    def test_changing_a_threshold_recomputes_with_no_network_and_no_write(self):
        write_rows(self.ledger, run_rows("a", 0, 0.8, 0.35))
        before = self.ledger.read_bytes()
        strict = json.loads(json.dumps(TABLES))
        for rule in strict["rescore"]["hive.qa.criterion"]["rules"]:
            if rule["id"] == "two_sided":
                rule["pass_min"] = 0.7
        with patch("socket.socket", side_effect=AssertionError("network used")):
            loose_report = rescore.rescore_qa(self.ledger, TABLES, {"a": "pass"})
            strict_report = rescore.rescore_qa(self.ledger, strict, {"a": "pass"})
        loose = loose_report["baselines"][0]["splits"]["all"]["rules"]["two_sided"]
        tight = strict_report["baselines"][0]["splits"]["all"]["rules"]["two_sided"]
        self.assertEqual((0, 0), (loose["false_fails"], loose["false_passes"]))
        self.assertEqual(1, tight["false_fails"])
        self.assertEqual(before, self.ledger.read_bytes())

    def test_module_imports_no_transport(self):
        tree = ast.parse((ROOT / "rescore.py").read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names |= {alias.name.split(".")[0] for alias in node.names}
            elif isinstance(node, ast.ImportFrom):
                names.add((node.module or "").split(".")[0])
        self.assertFalse(names & {"jev_transport", "judge", "urllib", "http", "socket", "requests", "shadow_qa"})

    def test_question_versions_are_separate_baselines_never_pooled(self):
        write_rows(self.ledger, run_rows("a", 0, 0.9, 0.1))
        write_rows(self.ledger, run_rows("a", 1, 0.1, 0.9, versions=("qv-new", "qv-misses", "qv-injection")))
        report = rescore.rescore_qa(self.ledger, TABLES, {"a": "pass"})
        self.assertFalse(report["baselines_compared_with_each_other"])
        self.assertEqual(2, len(report["baselines"]))
        self.assertEqual([1, 1], [b["runs"] for b in report["baselines"]])
        self.assertEqual([0, 0], [b["repeatability"]["one_sided"]["decision_flips"] for b in report["baselines"]])
        self.assertEqual([0, 1], sorted(
            b["splits"]["all"]["rules"]["one_sided"]["false_fails"] for b in report["baselines"]))

    def test_downstream_outcomes_are_not_truth_for_qa(self):
        write_rows(self.ledger, run_rows("a", 0, 0.9, 0.1))
        write_rows(self.ledger, [{"v": 1, "kind": "outcome", "ts": "t", "decision_id": "a",
                                  "label": {"result": "fail"}, "source": "downstream"}])
        report = rescore.rescore_qa(self.ledger, TABLES)
        self.assertEqual(0, report["baselines"][0]["labeled_items"])
        self.assertEqual({"downstream": 1}, report["counters"]["excluded_outcomes_by_source"])

    def test_human_label_outranks_override_and_scores_the_item(self):
        write_rows(self.ledger, run_rows("a", 0, 0.9, 0.1))
        write_rows(self.ledger, [
            {"v": 1, "kind": "outcome", "ts": "2", "decision_id": "a", "label": {"result": "pass"}, "source": "operator-override"},
            {"v": 1, "kind": "outcome", "ts": "1", "decision_id": "a", "label": {"result": "fail"}, "source": "human-label"},
        ])
        report = rescore.rescore_qa(self.ledger, TABLES)
        self.assertEqual(1, report["baselines"][0]["splits"]["all"]["rules"]["two_sided"]["false_passes"])
        self.assertEqual("ledger-outcomes", report["truth_class"])

    def test_small_or_authored_sets_say_they_are_too_thin(self):
        write_rows(self.ledger, run_rows("a", 0, 0.9, 0.1))
        reasons = rescore.rescore_qa(self.ledger, TABLES, {"a": "pass"})["baselines"][0]["too_thin_to_conclude"]
        self.assertEqual(3, len(reasons))
        self.assertIn("bench-author", reasons[0])
        enough = rescore._thin_reasons("ledger-outcomes", 200, 100, TABLES["targets"])
        self.assertEqual([], enough)

    def test_a_request_missing_a_half_or_with_an_error_is_counted_not_scored(self):
        rows = run_rows("a", 0, 0.9, 0.1)
        write_rows(self.ledger, rows[:1])
        errored = run_rows("b", 0, 0.9, 0.1)
        errored[1]["error"] = "transport-error"
        write_rows(self.ledger, errored)
        counters = rescore.rescore_qa(self.ledger, TABLES)["counters"]
        self.assertEqual(2, counters["incomplete_requests"])
        self.assertEqual(1, counters["error_rows"])

    def test_changed_evidence_drops_the_run(self):
        rows = run_rows("a", 0, 0.9, 0.1, state_hash=ledger.sha256_of({"evidence": "original"}))
        rows[0]["state_ref"] = "evidence/a.json"
        write_rows(self.ledger, rows)
        (self.root / "evidence").mkdir()
        (self.root / "evidence" / "a.json").write_text('{"evidence":"changed"}', encoding="utf-8")
        report = rescore.rescore_qa(self.ledger, TABLES, {"a": "pass"})
        self.assertEqual(1, report["counters"]["evidence"]["mismatch"])
        self.assertEqual([], report["baselines"])

    def test_recorded_answer_that_disagrees_with_its_probability_is_reported(self):
        rows = run_rows("a", 0, 0.9, 0.1)
        rows[1]["answer"] = {"result": "fail"}
        write_rows(self.ledger, rows)
        self.assertEqual(1, rescore.rescore_qa(self.ledger, TABLES)["counters"]["fidelity_mismatches"])

    def test_injection_flag_is_reported_and_changes_no_decision(self):
        write_rows(self.ledger, run_rows("a", 0, 0.9, 0.1, injection=0.98))
        baseline = rescore.rescore_qa(self.ledger, TABLES, {"a": "pass"})["baselines"][0]
        self.assertEqual(["a"], baseline["injection_flagged_items"])
        self.assertEqual(1, baseline["splits"]["all"]["rules"]["two_sided"]["accuracy"]["k"])

    def test_review_rows_replay_and_abstain_floor(self):
        def row(identifier, top, p):
            others = {"ACCEPT": 0.0, "PARTIAL": 0.0, "DECLINE": 0.0}
            others[top] = p
            return {"v": 1, "kind": "decision", "ts": identifier, "decision_id": identifier,
                    "surface": "hive.review.finding", "judge": "jev", "error": None,
                    "question_version": "qv", "answer": {"result": top}, "probabilities": others}
        write_rows(self.ledger, [row("a", "ACCEPT", 0.9), row("b", "DECLINE", 0.51)])
        write_rows(self.ledger, [
            {"v": 1, "kind": "outcome", "ts": "t", "decision_id": "a", "label": {"result": "ACCEPT"}, "source": "downstream"},
            {"v": 1, "kind": "outcome", "ts": "t", "decision_id": "b", "label": {"result": "ACCEPT"}, "source": "downstream"},
        ])
        baseline = rescore.rescore_review(self.ledger, TABLES)["baselines"][0]
        self.assertEqual(0, baseline["replay_mismatches"])
        self.assertEqual(0.5, baseline["rules"]["top_choice"]["agreement"]["rate"])
        self.assertEqual((1, 1), (baseline["rules"]["top_choice_min_0.7"]["decided"],
                                  baseline["rules"]["top_choice_min_0.7"]["abstained"]))
        self.assertEqual(1.0, baseline["rules"]["top_choice_min_0.7"]["agreement"]["rate"])


class FrozenBenchTests(unittest.TestCase):
    """Reproduces the numbers reported in docs/judgment-two-sided-qa.md with no key."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.ledger = Path(self.temporary.name) / "ledger.jsonl"
        write_rows(self.ledger, frozen_rows())
        labels = {item["id"]: item["label"] for item in json.loads(BENCH.read_text(encoding="utf-8"))["items"]}
        self.report = rescore.rescore_qa(self.ledger, TABLES, labels)
        self.baseline = self.report["baselines"][0]

    def test_bench_is_the_frozen_one(self):
        digest = hashlib.sha256(BENCH.read_bytes().replace(b"\r\n", b"\n")).hexdigest()
        self.assertEqual("9e1a455f7a96b8cc021ac7aca50c8a054c7c9e62e39d71478867e32198ff3a0a", digest)

    def test_trace_used_the_committed_question_wording(self):
        header = json.loads(TRACE.read_text(encoding="utf-8").splitlines()[0])
        questions = json.loads((ROOT / "qa_two_sided_questions.json").read_text(encoding="utf-8"))
        self.assertEqual({qid: ledger.sha256_of(question) for qid, question in questions.items()},
                         header["question_versions"],
                         "A wording change is a new baseline: run a new trace, don't edit this pin.")

    def test_reported_counts_reproduce(self):
        rules = self.baseline["splits"]["all"]["rules"]
        self.assertEqual((80, 800), (self.baseline["items"], self.baseline["runs"]))
        self.assertEqual((3, 0, 77), (rules["one_sided"]["false_passes"], rules["one_sided"]["false_fails"],
                                      rules["one_sided"]["accuracy"]["k"]))
        self.assertEqual((1, 0, 79), (rules["two_sided"]["false_passes"], rules["two_sided"]["false_fails"],
                                      rules["two_sided"]["accuracy"]["k"]))
        self.assertAlmostEqual(0.049, rules["one_sided"]["ece"], places=3)
        self.assertAlmostEqual(0.061, rules["two_sided"]["ece"], places=3)
        self.assertEqual((2, 0), (rules["two_sided_band"]["abstained"], rules["two_sided_band"]["false_passes"]))
        pair = self.baseline["splits"]["all"]["paired"][0]
        self.assertEqual((0, 2, 0.5), (pair["a_right_b_wrong"], pair["b_right_a_wrong"], pair["sign_test_p"]))
        held = self.baseline["splits"]["heldout"]["rules"]
        self.assertEqual((44, 43, 43), (held["one_sided"]["n"], held["one_sided"]["accuracy"]["k"],
                                        held["two_sided"]["accuracy"]["k"]))

    def test_repeatability_and_injection_reproduce(self):
        repeated = self.baseline["repeatability"]
        self.assertEqual((0, 0.1), (repeated["one_sided"]["decision_flips"], repeated["one_sided"]["jitter_max"]))
        self.assertEqual((0, 0.26), (repeated["two_sided"]["decision_flips"], repeated["two_sided"]["jitter_max"]))
        self.assertEqual(4, repeated["two_sided_band"]["decision_flips"])
        self.assertEqual(["b41", "b57"], self.baseline["injection_flagged_items"])

    def test_report_refuses_to_conclude(self):
        self.assertEqual("bench-author", self.report["truth_class"])
        self.assertEqual(3, len(self.baseline["too_thin_to_conclude"]))
        self.assertIn("TOO THIN", rescore.render_qa(self.report))


if __name__ == "__main__":
    unittest.main()
