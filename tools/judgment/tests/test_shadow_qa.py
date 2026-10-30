"""The two-sided shadow runner: blind payloads, judge.py's gates, resume. Network mocked."""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import judge  # noqa: E402
import ledger  # noqa: E402
import rescore  # noqa: E402
import shadow_qa  # noqa: E402


QIDS = ("hive.qa.criterion.meets", "hive.qa.criterion.misses", "hive.qa.criterion.injection")
KEY = "synthetic-test-key-value"


def item(identifier="i1", **changes):
    value = {
        "id": identifier, "category": "secret-category",
        "criterion": {"number": 1, "description": "GET /x returns 200.", "kind": "pass_fail"},
        "evidence": "curl localhost/x returned 200.", "evidence_refs": ["qa-worker-1#1"],
        "label": "fail", "why": "secret why",
    }
    value.update(changes)
    return value


def response(meets=0.9, misses=0.2, injection=0.01):
    return {"model": "synthetic-model", "usage": {"input_tokens": 1, "output_tokens": 1}, "answers": {
        QIDS[0]: {"type": "noul", "noul": meets}, QIDS[1]: {"type": "noul", "noul": misses},
        QIDS[2]: {"type": "noul", "noul": injection}}}


class ShadowQaTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.ledger = self.root / "ledger.jsonl"
        self.dictionary = self.root / "dictionary.json"
        self.dictionary.write_text(json.dumps(["Zebulon Quartermaine"]), encoding="utf-8")
        env = patch.dict(os.environ, {"TYPESAFE_API_KEY": KEY,
                                      "JUDGMENT_KEY_FILE": str(self.root / "absent")})
        env.start()
        self.addCleanup(env.stop)
        identity = patch.object(judge, "_project_identity", return_value="rdfitted/hive-manager")
        identity.start()
        self.addCleanup(identity.stop)

    def run_items(self, items, *, send_response=None, side_effect=None, **kwargs):
        kwargs.setdefault("runs", 1)
        kwargs.setdefault("dictionary", str(self.dictionary))
        self.slept = []
        kwargs.setdefault("sleep", self.slept.append)
        mock_args = {"side_effect": side_effect} if side_effect else {"return_value": send_response or response()}
        with patch.object(judge.jev_transport, "send", **mock_args) as send:
            summary = shadow_qa.run(items, self.ledger, **kwargs)
        return summary, send

    def rows(self):
        return list(ledger.read_records([self.ledger])) if self.ledger.exists() else []

    def test_one_request_three_rows_on_their_own_surfaces(self):
        summary, send = self.run_items([item()])
        send.assert_called_once()
        self.assertEqual(1, summary["recorded"])
        rows = {row["surface"]: row for row in self.rows()}
        self.assertEqual({"hive.qa.criterion", "hive.qa.criterion.misses", "hive.qa.criterion.injection"}, set(rows))
        self.assertEqual(1, len({row["request_id"] for row in rows.values()}))
        self.assertEqual(1, len({row["state_hash"] for row in rows.values()}))
        self.assertEqual({"result": "pass"}, rows["hive.qa.criterion"]["answer"])
        self.assertEqual({"result": "pass"}, rows["hive.qa.criterion.misses"]["answer"])
        self.assertEqual({"result": "false"}, rows["hive.qa.criterion.injection"]["answer"])
        for row in rows.values():
            self.assertEqual(("shadow", "none", {}, "i1"),
                             (row["mode"], row["routed"], row["sampling"], row["source_decision_id"]))

    def test_a_yes_on_misses_records_as_fail(self):
        self.run_items([item()], send_response=response(misses=0.8))
        row = next(r for r in self.rows() if r["surface"] == "hive.qa.criterion.misses")
        self.assertEqual({"result": "fail"}, row["answer"])
        self.assertEqual({"fail": 0.8, "pass": pytest_approx(0.2)}, row["probabilities"])

    def test_companions_only_apply_to_the_qa_surface(self):
        spec = {"type": "noul", "instructions": "Does `s` match?"}
        answer, _ = judge._answer("ui.slop", {"model": "m", "answers": {QIDS[1]: {"type": "noul", "noul": 0.1}}}, QIDS[1], spec)
        self.assertEqual({"result": "drop"}, answer)

    def test_payload_is_observations_only(self):
        _, send = self.run_items([item(result="LEAK-RESULT", rationale="LEAK-RATIONALE", answer="LEAK-ANSWER",
                                       incumbent_conclusion={"result": "LEAK-INCUMBENT"})])
        payload = json.loads(send.call_args.args[0])
        self.assertEqual({"criterion", "evidence", "evidence_refs"}, set(payload["state"]))
        self.assertEqual(set(QIDS), set(payload["questions"]))
        sent = send.call_args.args[0].decode("utf-8")
        for secret in ("LEAK-", "secret-category", "secret why", '"label"', '"fail"'):
            self.assertNotIn(secret, sent)

    def test_verdict_language_in_evidence_is_never_sent(self):
        for text in ("CRITERION 2: FAIL - no pulse", "QA_VERDICT: PASS", "The verdict stands.",
                     "Rationale: looks right.", "This requirement is met.", "the criterion failed here"):
            with self.subTest(text=text):
                summary, send = self.run_items([item(evidence=text)])
                send.assert_not_called()
                self.assertEqual({"i1": "verdict-text"}, summary["skipped"])
        self.assertEqual([], self.rows())

    def test_ordinary_observation_language_is_sent(self):
        summary, send = self.run_items([item(evidence="cargo test: 12 passed, 0 failed. Run npm ci first.")])
        send.assert_called_once()
        self.assertEqual({}, summary["skipped"])

    def test_scored_and_measured_criteria_are_not_sent(self):
        for kind in ("scored", "measured", None):
            with self.subTest(kind=kind):
                summary, send = self.run_items([item(criterion={"description": "d", "kind": kind})])
                send.assert_not_called()
                self.assertEqual({"i1": "not-pass-fail"}, summary["skipped"])

    def test_dry_run_needs_no_key_and_makes_no_call(self):
        with patch.dict(os.environ, {"TYPESAFE_API_KEY": ""}):
            summary, send = self.run_items([item()], dry_run=True)
        send.assert_not_called()
        self.assertEqual({"dry-run": 1}, summary["statuses"])
        self.assertEqual(["dry-run"] * 3, [row["error"] for row in self.rows()])

    def test_other_projects_are_refused_by_the_existing_gate(self):
        with patch.object(judge, "_project_identity", return_value="synthetic/client-project"):
            summary, send = self.run_items([item()])
        send.assert_not_called()
        self.assertEqual({"i1": "egress-denied"}, summary["skipped"])

    def test_surviving_name_is_blocked_and_never_sent(self):
        summary, send = self.run_items([item(evidence="Zebulon Quartermaine saw a 200.")])
        send.assert_not_called()
        self.assertEqual({"i1": "redaction-blocked"}, summary["skipped"])
        self.assertNotIn("Zebulon", self.ledger.read_text(encoding="utf-8"))

    def test_missing_dictionary_fails_closed(self):
        with patch.dict(os.environ, {"JUDGMENT_REDACT_PROFILE_ROOT": ""}):
            summary, send = self.run_items([item()], dictionary=None)
        send.assert_not_called()
        self.assertEqual({"i1": "redaction-blocked"}, summary["skipped"])

    def test_transport_failure_backs_off_then_completes(self):
        failure = judge.jev_transport.TransportError("http-429")
        summary, send = self.run_items([item()], side_effect=[failure, failure, response()])
        self.assertEqual(3, send.call_count)
        self.assertEqual([5.0, 10.0], self.slept)
        self.assertEqual({"transport-error": 2, "recorded": 1}, summary["statuses"])
        self.assertIsNone(summary["stopped"])

    def test_backoff_is_bounded(self):
        failure = judge.jev_transport.TransportError("http-429")
        summary, _ = self.run_items([item()], side_effect=[failure] * 50)
        self.assertEqual("backoff-exhausted", summary["stopped"])
        self.assertEqual(shadow_qa.BACKOFF_TOTAL_S, sum(self.slept))

    def test_a_wording_change_does_not_reuse_the_old_baselines_runs(self):
        self.run_items([item()])
        changed = json.loads(shadow_qa.DEFAULT_QUESTIONS.read_text(encoding="utf-8"))
        changed[QIDS[0]]["instructions"] = "Does `evidence` demonstrate `criterion.description`?"
        reworded = self.root / "reworded.json"
        reworded.write_text(json.dumps(changed), encoding="utf-8")
        summary, send = self.run_items([item()], questions=reworded)
        send.assert_called_once()
        self.assertEqual(1, summary["recorded"])
        summary, send = self.run_items([item()], questions=reworded)
        send.assert_not_called()

    def test_resume_and_request_cap(self):
        summary, send = self.run_items([item("a"), item("b")], runs=2, max_requests=3)
        self.assertEqual((3, "max-requests"), (send.call_count, summary["stopped"]))
        summary, send = self.run_items([item("a"), item("b")], runs=2)
        self.assertEqual(1, send.call_count)
        self.assertIsNone(summary["stopped"])
        summary, send = self.run_items([item("a"), item("b")], runs=2)
        send.assert_not_called()

    def test_key_is_in_no_row_and_no_output(self):
        output = io.StringIO()
        items = self.root / "items.json"
        items.write_text(json.dumps({"items": [item()]}), encoding="utf-8")
        with patch.object(judge.jev_transport, "send", return_value=response()), redirect_stdout(output):
            code = shadow_qa.main(["--items", str(items), "--ledger", str(self.ledger), "--runs", "1",
                                   "--redact-dictionary", str(self.dictionary)])
        self.assertEqual(0, code)
        self.assertNotIn(KEY, output.getvalue())
        self.assertNotIn(KEY, self.ledger.read_text(encoding="utf-8"))

    def test_default_ledger_is_refused(self):
        items = self.root / "items.json"
        items.write_text(json.dumps({"items": [item()]}), encoding="utf-8")
        with patch.dict(os.environ, {"JUDGMENT_LEDGER": str(self.ledger)}), \
                patch.object(judge.jev_transport, "send") as send, redirect_stdout(io.StringIO()):
            code = shadow_qa.main(["--items", str(items), "--ledger", str(self.ledger)])
        self.assertEqual(1, code)
        send.assert_not_called()

    def test_rescore_reads_what_the_runner_writes(self):
        self.run_items([item()], send_response=response(meets=0.8, misses=0.7))
        tables = json.loads((ROOT / "decision-tables.json").read_text(encoding="utf-8"))
        baseline = rescore.rescore_qa(self.ledger, tables, {"i1": "fail"})["baselines"][0]
        rules = baseline["splits"]["all"]["rules"]
        self.assertEqual(1, rules["one_sided"]["false_passes"])
        self.assertEqual(0, rules["two_sided"]["false_passes"])

    def test_scorecard_scores_only_the_meets_row_on_the_primary_surface(self):
        self.run_items([item()], send_response=response(meets=0.9, misses=0.9))
        primary = [row for row in self.rows() if row["surface"] == "hive.qa.criterion"]
        self.assertEqual(1, len(primary))
        self.assertEqual("hive.qa.criterion.meets", primary[0]["question_id"])


def pytest_approx(value):
    class Near(float):
        def __eq__(self, other):
            return abs(other - value) < 1e-9
        __hash__ = None
    return Near(value)


if __name__ == "__main__":
    unittest.main()
