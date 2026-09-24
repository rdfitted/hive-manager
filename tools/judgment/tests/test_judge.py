"""Offline tests for the gated on-demand shadow judge."""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import MagicMock, patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import audit  # noqa: E402
import judge  # noqa: E402
import jev_transport  # noqa: E402
import ledger  # noqa: E402
import redaction  # noqa: E402


class JudgeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.ledger = self.root / "ledger.jsonl"
        self.policy = ROOT / "egress-policy.json"
        self.dictionary = self.root / "dictionary.json"
        self.dictionary.write_text(json.dumps(["Zebulon Quartermaine"]), encoding="utf-8")
        self.subject = self._file("subject.json", {"session_id": "synthetic-session", "criterion_id": "c1"})
        self.observations = self._file("observations.json", {"observed": "Synthetic evidence confirms behavior."})
        self.question = self._file("question.json", {
            "type": "noul", "instructions": "Does the observed evidence satisfy the criterion?",
            "criteria": {"true": "It does.", "false": "It does not."},
        })
        self.key = patch.dict(os.environ, {"TYPESAFE_API_KEY": "synthetic-test-key"})
        self.key.start()
        self.addCleanup(self.key.stop)

    def _file(self, name, value):
        path = self.root / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def _args(self, *, surface="hive.qa.criterion", policy=None, dry=False, question=None,
              observations=None):
        args = ["--ledger", str(self.ledger), "--policy", str(policy or self.policy),
                "--redact-dictionary", str(self.dictionary), "ask",
                "--surface", surface, "--subject-ref", str(self.subject),
                "--observations", str(observations or self.observations),
                "--question-id", "q1", "--question", str(question or self.question)]
        if dry:
            args.append("--dry-run")
        return args

    def _run(self, args, *, project="rdfitted/hive-manager", provider=None):
        output = io.StringIO()
        response = provider or {"model": "jev-1.13.0", "answers": {"q1": {
            "type": "noul", "noul": 0.75,
        }}, "usage": {"input_tokens": 12, "output_tokens": 3}}
        with patch.object(judge, "_project_identity", return_value=project), \
                patch.object(judge.jev_transport, "send", return_value=response) as send, \
                redirect_stdout(output):
            code = judge.main(args)
        result = json.loads(output.getvalue()) if output.getvalue() else None
        return code, result, send

    def _rows(self):
        return list(ledger.read_records([self.ledger]))

    def test_egress_denied_surface_writes_row_without_send(self):
        policy = json.loads(self.policy.read_text(encoding="utf-8"))
        del policy["surfaces"]["hive.qa.criterion"]
        denied_policy = self._file("denied-policy.json", policy)
        code, result, send = self._run(self._args(policy=denied_policy))
        self.assertEqual((3, "egress-denied", False), (code, result["status"], result["sent"]))
        send.assert_not_called()
        self.assertEqual("egress-denied", self._rows()[-1]["error"])

    def test_egress_denied_project_writes_row_without_send(self):
        code, result, send = self._run(self._args(), project="synthetic/other")
        self.assertEqual((3, "egress-denied", False), (code, result["status"], result["sent"]))
        send.assert_not_called()
        self.assertEqual(1, len(self._rows()))
        self.assertIsNone(self._rows()[-1]["state_ref"])

    def test_redaction_blocks_a_synthetic_dictionary_name_and_email(self):
        observations = self._file("named.json", {"observed": "Zebulon Quartermaine reviewed this."})
        code, result, send = self._run(self._args(observations=observations))
        self.assertEqual((3, "redaction-blocked"), (code, result["status"]))
        send.assert_not_called()
        self.assertIsNone(self._rows()[-1]["state_ref"])
        self.assertNotIn("Zebulon", self.ledger.read_text(encoding="utf-8"))
        self.assertEqual({"hit_count": 1, "blocked": True,
                          "reason_classes": ["dictionary"]}, result["redaction"])
        self.assertEqual(result["redaction"], self._rows()[-1]["redaction"])
        observations = self._file("email.json", {"observed": "bot@example.invalid"})
        code, result, send = self._run(self._args(observations=observations))
        self.assertEqual((3, "redaction-blocked"), (code, result["status"]))
        send.assert_not_called()

    def test_blocked_redaction_reports_classes_without_matched_text(self):
        observations = self._file("reason-observations.json", {
            "observed": "Zebulon Quartermaine used bot@example.invalid"
        })
        code, result, send = self._run(self._args(observations=observations))
        self.assertEqual(3, code)
        send.assert_not_called()
        self.assertEqual(["dictionary", "email"], result["redaction"]["reason_classes"])
        self.assertGreaterEqual(result["redaction"]["hit_count"], 2)
        self.assertEqual(result["redaction"], self._rows()[-1]["redaction"])
        for forbidden in ("Zebulon", "Quartermaine", "bot@example.invalid"):
            self.assertNotIn(forbidden, json.dumps(result))
            self.assertNotIn(forbidden, self.ledger.read_text(encoding="utf-8"))

    def test_blind_payload_rejects_answer_rationale_severity_verdict_and_measured(self):
        expected = {"state": {"observed": "Synthetic evidence confirms behavior."},
                    "model": "jev-latest", "questions": {"q1": {
                        "type": "noul", "instructions": "Does the observed evidence satisfy the criterion?",
                        "criteria": {"true": "It does.", "false": "It does not."},
                    }}}
        code, result, send = self._run(self._args())
        self.assertEqual(0, code)
        self.assertEqual("recorded", result["status"])
        self.assertEqual(judge._json_bytes(expected), send.call_args.args[0])
        for name in ("answer", "rationale", "severity", "verdict", "measured"):
            with self.subTest(name=name):
                observations = self._file(f"{name}.json", {"observed": {name: "hidden"}})
                code, result, send = self._run(self._args(observations=observations))
                self.assertEqual(2, code)
                self.assertEqual("invalid-input", result["status"])
                send.assert_not_called()

    def test_dry_run_no_key_size_and_missing_dictionary_do_not_send(self):
        code, result, send = self._run(self._args(dry=True))
        self.assertEqual((0, "dry-run"), (code, result["status"]))
        send.assert_not_called()
        with patch.dict(os.environ, {"TYPESAFE_API_KEY": ""}):
            code, result, send = self._run(self._args())
        self.assertEqual((0, "no-key"), (code, result["status"]))
        send.assert_not_called()
        huge = self._file("huge.json", {"observed": "x" * (32 * 1024)})
        code, result, send = self._run(self._args(observations=huge))
        self.assertEqual((3, "size-blocked"), (code, result["status"]))
        send.assert_not_called()
        self.dictionary.unlink()
        code, result, send = self._run(self._args())
        self.assertEqual((3, "redaction-blocked"), (code, result["status"]))
        send.assert_not_called()
        self.assertEqual(["dictionary-unavailable"], result["redaction"]["reason_classes"])
        self.assertEqual(result["redaction"], self._rows()[-1]["redaction"])
        self.assertEqual(4, len(self._rows()))

    def test_provider_mapping_and_outcome_audit_join(self):
        code, result, _ = self._run(self._args())
        self.assertEqual(0, code)
        row = self._rows()[-1]
        self.assertEqual({"result": "pass"}, row["answer"])
        self.assertEqual("jev-1.13.0", row["model"])
        self.assertEqual({"input_tokens": 12, "output_tokens": 3}, row["usage"])
        self.assertIsInstance(row["latency_ms"], int)
        self.assertEqual("noul-0.5", row["threshold_id"])
        label = self._file("label.json", {"result": "pass"})
        code, outcome, send = self._run([
            "--ledger", str(self.ledger), "outcome", "--decision-id", result["decision_id"],
            "--label", str(label), "--source", "human-label",
        ])
        self.assertEqual((0, "recorded"), (code, outcome["status"]))
        send.assert_not_called()
        self.assertEqual(row["decision_id"], self._rows()[-1]["decision_id"])
        tables = self._file("tables.json", {"surfaces": {"hive.qa.criterion": {
            "mode": "shadow", "question_type": "multiclass",
        }}})
        with redirect_stdout(io.StringIO()) as output:
            self.assertEqual(0, audit.main(["--ledger", str(self.ledger),
                                            "--tables", str(tables), "--no-retrievals",
                                            "--no-history", "--json"]))
        group = next(group for group in json.loads(output.getvalue())["judges"]
                     if group["surface"] == "hive.qa.criterion")
        self.assertEqual(1, group["joined"])

        review = self._file("review.json", {"type": "choice", "instructions": "Classify the finding.",
                                            "criteria": {"ACCEPT": None, "PARTIAL": None, "DECLINE": None}})
        provider = {"model": "jev-1.13.1", "answers": {"q1": {"type": "choice",
                    "choice": "PARTIAL", "probabilities": {"ACCEPT": 0.1, "PARTIAL": 0.8,
                    "DECLINE": 0.1}, "confidence": 0.8}},
                    "usage": {"input_tokens": 4, "output_tokens": 2}}
        code, result, _ = self._run(self._args(surface="hive.review.finding", question=review), provider=provider)
        self.assertEqual((0, {"result": "PARTIAL"}), (code, result["answer"]))
        self.assertEqual(0.8, self._rows()[-1]["confidence"])

    def test_ledger_failure_is_exit_five(self):
        with patch.object(judge.ledger, "record_decision", return_value=None):
            code, result, send = self._run(self._args(dry=True))
        self.assertEqual((5, "ledger-error"), (code, result["status"]))
        send.assert_not_called()

    def test_project_identity_accepts_only_github_origin_and_cannot_be_overridden(self):
        self.assertEqual("rdfitted/hive-manager", judge._project_identity())
        self.assertEqual("owner/repo", judge.GITHUB_REMOTE.fullmatch(
            "git@github.com:Owner/Repo.git").group(1).lower() + "/" +
            judge.GITHUB_REMOTE.fullmatch("git@github.com:Owner/Repo.git").group(2).lower())
        self.assertIsNone(judge.GITHUB_REMOTE.fullmatch("https://example.invalid/owner/repo.git"))
        subject = self._file("spoof.json", {"project": "synthetic/other"})
        self.subject = subject
        code, result, _ = self._run(self._args())
        self.assertEqual(0, code)
        self.assertEqual("rdfitted/hive-manager", self._rows()[-1]["project"])

    def test_redaction_case_rules_and_incomplete_scan(self):
        dictionary = ["Zebulon Quartermaine", "Bo", {"term": "Acme Fabrication", "kind": "organization"}]
        for text in ("zebulon quartermaine", "ACME FABRICATION", "Bo", "bot@example.invalid"):
            with self.subTest(text=text):
                value, report = redaction.redact({"text": text}, "names", dictionary)
                self.assertIsNone(value)
                self.assertTrue(report["blocked"])
                self.assertGreater(report["hit_count"], 0)
                self.assertNotIn(text, json.dumps(report))
        value, report = redaction.redact({"text": "bo"}, "names", dictionary)
        self.assertEqual({"text": "bo"}, value)
        self.assertFalse(report["blocked"])
        for mode, items, payload in (("unknown", dictionary, {}), ("names", None, {}),
                                     ("names", dictionary, {"bad": object()})):
            value, report = redaction.redact(payload, mode, items)
            self.assertIsNone(value)
            self.assertTrue(report["blocked"])
        profiles = self.root / "profiles"
        (profiles / "Synthetic-Fabrication").mkdir(parents=True)
        derived = redaction.load_dictionary(None, profiles)
        value, report = redaction.redact({"text": "synthetic fabrication"}, "names", derived)
        self.assertIsNone(value)
        self.assertTrue(report["blocked"])

    def test_transport_retries_only_rate_limit_and_overload(self):
        response = MagicMock()
        response.__enter__.return_value = response
        response.status = 200
        response.read.return_value = b'{"model":"jev-1.13.0"}'
        limited = jev_transport.error.HTTPError(jev_transport.ENDPOINT, 429, "", None, None)
        overloaded = jev_transport.error.HTTPError(jev_transport.ENDPOINT, 529, "", None, None)
        with patch.object(jev_transport.request, "urlopen", side_effect=[limited, overloaded, response]) as call, \
                patch.object(jev_transport.time, "sleep") as sleep:
            self.assertEqual("jev-1.13.0", jev_transport.send(b"{}", "synthetic-key")["model"])
        self.assertEqual(3, call.call_count)
        self.assertEqual(2, sleep.call_count)
        denied = jev_transport.error.HTTPError(jev_transport.ENDPOINT, 401, "", None, None)
        with patch.object(jev_transport.request, "urlopen", side_effect=denied) as call:
            with self.assertRaisesRegex(jev_transport.TransportError, "http-401"):
                jev_transport.send(b"{}", "synthetic-key")
        self.assertEqual(1, call.call_count)


if __name__ == "__main__":
    unittest.main()
