"""Synthetic review adjudications through judge and the canonical auditor."""

from __future__ import annotations

import io
import json
import os
import subprocess
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


def _blind_observation(finding: dict) -> dict:
    """Take only primary evidence; the Reconciler's conclusion stays local."""
    return {
        "finding_text": finding["text"],
        "citation": f'{finding["path"]}:{finding["line"]}',
        "reviewed_code_window": finding["code_window"],
    }


class ReviewLedgerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.ledger = self.root / "review-ledger.jsonl"
        self.dictionary = self._json("redaction-dictionary.json", ["Zebulon Quartermaine"])
        self.question = self._json("question.json", {
            "type": "choice",
            "instructions": "Classify this independent external finding from the cited code.",
            "criteria": {"ACCEPT": "Whole finding is valid.",
                         "PARTIAL": "Only a supported subset is valid.",
                         "DECLINE": "Finding is unsupported."},
        })
        self.findings = [
            {"id": f"f{index}", "text": f"Synthetic finding {index} about branch handling.",
             "path": f"src/sample_{index}.rs", "line": index * 10,
             "code_window": f"fn sample_{index}() {{ /* synthetic */ }}",
             "severity": "MAJOR", "rationale": "Reconciler conclusion stays local.",
             "disposition": "DECLINE" if index == 5 else "ACCEPT"}
            for index in range(1, 6)
        ]
        self.artifact = {
            "schema_version": "hive.review-adjudications/v1", "pr_number": 42,
            "round": 1,
            "findings": [
                {"finding_id": item["id"], "thread_id_or_url": f'thread-{item["id"]}',
                 "path": item["path"], "line": item["line"],
                 "reviewed_sha": "a" * 40, "disposition": item["disposition"]}
                for item in self.findings
            ],
        }
        self._json("review-round-1.json", self.artifact)

    def _json(self, name: str, value: object) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def _judge(self, command: list[str], *, response: dict | None = None):
        stdout = io.StringIO()
        with patch.object(judge, "_project_identity", return_value="rdfitted/hive-manager"), \
                patch.object(judge.jev_transport, "send", return_value=response) as send, \
                patch.dict(os.environ, {"TYPESAFE_API_KEY": "synthetic-test-key"}), \
                redirect_stdout(stdout):
            code = judge.main([
                "--ledger", str(self.ledger), "--redact-dictionary", str(self.dictionary),
                *command,
            ])
        return code, json.loads(stdout.getvalue()), send

    def _subject(self, entry: dict) -> Path:
        return self._json(f'{entry["finding_id"]}-subject.json', {
            "repo": "rdfitted/hive-manager", "pr_number": self.artifact["pr_number"],
            "round": self.artifact["round"], "finding_id": entry["finding_id"],
            "thread_id_or_url": entry["thread_id_or_url"], "path": entry["path"],
            "line": entry["line"], "reviewed_sha": entry["reviewed_sha"],
        })

    def test_five_incumbents_shadows_and_outcomes_have_exact_agreement(self):
        self.assertEqual(["ACCEPT"] * 4 + ["DECLINE"],
                         [entry["disposition"] for entry in self.artifact["findings"]])
        incumbent_ids = []
        sent_payloads = []
        sent_bytes = []
        for finding, entry in zip(self.findings, self.artifact["findings"]):
            subject = self._subject(entry)
            observation = self._json(f'{finding["id"]}-observation.json',
                                     _blind_observation(finding))
            answer = self._json(f'{finding["id"]}-answer.json',
                                {"result": entry["disposition"]})
            code, incumbent, send = self._judge([
                "record", "--surface", "hive.review.finding", "--subject-ref", str(subject),
                "--answer", str(answer), "--question-id", "review_finding_disposition",
                "--model", "synthetic-reconciler",
            ])
            self.assertEqual((0, "recorded", False),
                             (code, incumbent["status"], incumbent["sent"]))
            send.assert_not_called()
            incumbent_ids.append(incumbent["decision_id"])

            shadow_label = "PARTIAL" if finding["id"] == "f4" else entry["disposition"]
            response = {"model": "jev-synthetic", "answers": {
                "review_finding_disposition": {
                    "type": "choice", "choice": shadow_label,
                    "probabilities": {label: (0.8 if label == shadow_label else 0.1)
                                      for label in ("ACCEPT", "PARTIAL", "DECLINE")},
                }}, "usage": {"input_tokens": 8, "output_tokens": 2}}
            code, shadow, send = self._judge([
                "ask", "--surface", "hive.review.finding", "--subject-ref", str(subject),
                "--observations", str(observation),
                "--question-id", "review_finding_disposition",
                "--question", str(self.question),
                "--source-decision-id", incumbent["decision_id"],
            ], response=response)
            self.assertEqual((0, "recorded", True, {"result": shadow_label}),
                             (code, shadow["status"], shadow["sent"], shadow["answer"]))
            self.assertEqual(1, send.call_count)
            sent_bytes.append(send.call_args.args[0])
            sent_payloads.append(json.loads(send.call_args.args[0]))

            tier = "reviewer-resolved" if finding["id"] == "f5" else "fixed"
            detail = ("thread_id=thread-f5 reply_id=reply-f5" if tier == "reviewer-resolved"
                      else f'fix_sha={"b" * 40} test=synthetic-{finding["id"]} RED=1 GREEN=1')
            label = self._json(f'{finding["id"]}-label.json',
                               {"result": entry["disposition"]})
            code, outcome, send = self._judge([
                "outcome", "--decision-id", incumbent["decision_id"],
                "--label", str(label), "--source", "downstream",
                "--note", f"evidence_tier={tier} {detail}",
            ])
            self.assertEqual((0, "recorded", incumbent["decision_id"]),
                             (code, outcome["status"], outcome["decision_id"]))
            send.assert_not_called()

        rows = list(ledger.read_records([self.ledger]))
        decisions = [row for row in rows if row["kind"] == "decision"]
        outcomes = [row for row in rows if row["kind"] == "outcome"]
        self.assertEqual((10, 5), (len(decisions), len(outcomes)))
        for index, incumbent_id in enumerate(incumbent_ids):
            incumbent, shadow = decisions[2 * index:2 * index + 2]
            self.assertEqual(("incumbent-llm", "jev"),
                             (incumbent["judge"], shadow["judge"]))
            self.assertEqual(incumbent_id, shadow["source_decision_id"])
            self.assertEqual(("shadow", "none"), (shadow["mode"], shadow["routed"]))
            self.assertEqual(incumbent_id, outcomes[index]["decision_id"])
            self.assertEqual("downstream", outcomes[index]["source"])
            self.assertIn("evidence_tier=reviewer-resolved" if index == 4 else "evidence_tier=fixed",
                          outcomes[index]["note"])
            expected_payload = {"state": _blind_observation(self.findings[index]),
                                "model": "jev-latest", "questions": {
                                    "review_finding_disposition": json.loads(
                                        self.question.read_text(encoding="utf-8"))}}
            self.assertEqual(judge._json_bytes(expected_payload), sent_bytes[index])
            self.assertEqual(_blind_observation(self.findings[index]), sent_payloads[index]["state"])
            self.assertNotIn("rationale", json.dumps(sent_payloads[index]))
            self.assertNotIn("MAJOR", json.dumps(sent_payloads[index]))

        completed = subprocess.run([
            sys.executable, str(ROOT / "audit.py"), "--ledger", str(self.ledger),
            "--tables", str(ROOT / "decision-tables.json"),
            "--no-retrievals", "--no-history", "--json",
        ], capture_output=True, text=True, check=True)
        report = json.loads(completed.stdout)
        groups = {group["judge"]: group for group in report["judges"]
                  if group["surface"] == "hive.review.finding"}
        self.assertEqual({"incumbent-llm", "jev"}, set(groups))
        self.assertEqual((5, 5, 1.0),
                         (groups["incumbent-llm"]["items"], groups["incumbent-llm"]["joined"],
                          groups["incumbent-llm"]["rolling_agreement"]))
        self.assertEqual((5, 5, 0.8),
                         (groups["jev"]["items"], groups["jev"]["joined"],
                          groups["jev"]["rolling_agreement"]))

    def test_review_blindness_rejects_rationale_and_bot_severity(self):
        finding = self.findings[0]
        subject = self._subject(self.artifact["findings"][0])
        expected = {"finding_text": finding["text"],
                    "citation": f'{finding["path"]}:{finding["line"]}',
                    "reviewed_code_window": finding["code_window"]}
        self.assertEqual(expected, _blind_observation(finding))
        for leak in ("rationale", "severity"):
            with self.subTest(leak=leak):
                observation = {**_blind_observation(finding), leak: finding[leak]}
                path = self._json(f"leaked-{leak}.json", observation)
                code, result, send = self._judge([
                    "ask", "--surface", "hive.review.finding", "--subject-ref", str(subject),
                    "--observations", str(path),
                    "--question-id", "review_finding_disposition",
                    "--question", str(self.question),
                ])
                self.assertEqual((2, "invalid-input"), (code, result["status"]))
                send.assert_not_called()
                self.assertFalse(self.ledger.exists())


if __name__ == "__main__":
    unittest.main()
