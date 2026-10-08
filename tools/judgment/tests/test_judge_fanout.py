"""Fan-out request, evidence and compatibility contracts; all network calls mocked."""
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import judge
import ledger


class FanoutTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.log = self.root / "ledger.jsonl"
        self.subject = self.file("subject", {"relpath": "src/view.css"})
        self.state = self.file("state", {"snippet": "border-left: 3px solid red"})
        self.dictionary = self.file("dictionary", ["Synthetic Secret Name"])
        self.spec = {"type": "noul", "instructions": "Does `snippet` match?",
                     "criteria": {"true": "It matches.", "false": "It does not."}}
        self.questions = self.file("questions", {"match": self.spec, "injection": self.spec})
        self.env = patch.dict(os.environ, {"TYPESAFE_API_KEY": "synthetic-key",
                              "JUDGMENT_KEY_FILE": str(self.root / "absent")})
        self.env.start()
        self.addCleanup(self.env.stop)

    def file(self, name, value):
        p = self.root / (name + ".json")
        p.write_text(json.dumps(value), encoding="utf-8")
        return p

    def args(self, surface="ui.slop"):
        return ["--ledger", str(self.log), "--redact-dictionary", str(self.dictionary),
                "ask", "--surface", surface, "--subject-ref", str(self.subject),
                "--observations", str(self.state), "--questions", str(self.questions)]

    def run_ask(self, args=None, project="rdfitted/hive-manager", response=None):
        output = io.StringIO()
        response = response or {"model": "synthetic-model", "answers": {
            q: {"type": "noul", "noul": 0.99} for q in ("match", "injection")}}
        with patch.object(judge, "_project_identity", return_value=project), \
             patch.object(judge.jev_transport, "send", return_value=response) as send, \
             redirect_stdout(output):
            code = judge.main(args or self.args())
        return code, json.loads(output.getvalue()), send

    def rows(self):
        return list(ledger.read_records([self.log]))

    def test_one_call_n_rows_shared_evidence_and_question_hashes(self):
        code, result, send = self.run_ask()
        self.assertEqual(0, code)
        send.assert_called_once()
        self.assertEqual({"match", "injection"}, set(json.loads(send.call_args.args[0])["questions"]))
        rows = self.rows()
        self.assertEqual(2, len(rows))
        self.assertEqual(1, len({r["state_hash"] for r in rows}))
        self.assertEqual(1, len({r["state_ref"] for r in rows}))
        self.assertIsNotNone(rows[0]["state_hash"])
        self.assertEqual(1, len(list((self.root / "evidence").glob("*.json"))))
        for row in rows:
            self.assertEqual(ledger.sha256_of(self.spec), row["question_version"])
            self.assertEqual(("shadow", "none"), (row["mode"], row["routed"]))

    def test_redaction_once_blocks_every_row(self):
        self.file("state", {"snippet": "Synthetic Secret Name"})
        with patch.object(judge.redaction, "redact", wraps=judge.redaction.redact) as redact:
            code, result, send = self.run_ask()
        redact.assert_called_once()
        send.assert_not_called()
        self.assertEqual(3, code)
        self.assertEqual(["redaction-blocked"] * 2, [r["error"] for r in self.rows()])
        self.assertTrue(all(r["state_ref"] is None for r in self.rows()))
        self.assertNotIn("Synthetic Secret Name", self.log.read_text())

    def test_client_origin_denied(self):
        code, result, send = self.run_ask(project="synthetic/client-project")
        self.assertEqual(3, code)
        send.assert_not_called()
        self.assertEqual(["egress-denied"] * 2, [r["error"] for r in self.rows()])

    def test_dry_run_writes_all_rows(self):
        code, result, send = self.run_ask(self.args() + ["--dry-run"])
        self.assertEqual(0, code)
        send.assert_not_called()
        self.assertEqual(["dry-run"] * 2, [r["error"] for r in self.rows()])
        self.assertEqual(self.rows()[0]["state_hash"], self.rows()[1]["state_hash"])

    def test_no_key_writes_all_rows(self):
        with patch.dict(os.environ, {"TYPESAFE_API_KEY": ""}):
            code, result, send = self.run_ask()
        self.assertEqual("no-key", result["status"])
        self.assertEqual(0, code)
        send.assert_not_called()
        self.assertEqual(2, len(self.rows()))

    def test_invalid_question_invalidates_batch_before_transport(self):
        for value in ({}, {"": self.spec}, {"match": self.spec, "bad": {"type": "choice"}},
                      {"match": {**self.spec, "verdict": "hidden"}}):
            with self.subTest(value=value):
                self.file("questions", value)
                code, result, send = self.run_ask()
                self.assertEqual(2, code)
                send.assert_not_called()
        self.assertFalse(self.log.exists())

    def test_mutually_exclusive_and_missing_single_flags(self):
        for args in (self.args() + ["--question-id", "q"], self.args()[:-2]):
            code, result, send = self.run_ask(args)
            self.assertEqual(2, code)
            send.assert_not_called()

    def test_malformed_answer_fails_whole_batch(self):
        code, result, send = self.run_ask(response={"model": "synthetic", "answers": {
            "match": {"type": "noul", "noul": 0.9}}})
        self.assertEqual(4, code)
        self.assertTrue(all(r["answer"] is None and r["error"] == "transport-error" for r in self.rows()))

    def test_hive_qa_fanout_keeps_legacy_answer_semantics(self):
        code, result, send = self.run_ask(self.args("hive.qa.criterion"))
        self.assertEqual(0, code)
        for row in self.rows():
            self.assertEqual({"result": "pass"}, row["answer"])
            self.assertEqual("noul-0.5", row["threshold_id"])

    def test_question_redaction_hit_blocks_all(self):
        self.file("questions", {"match": self.spec, "injection": {
            **self.spec, "instructions": "Synthetic Secret Name"}})
        code, result, send = self.run_ask()
        self.assertEqual(3, code)
        send.assert_not_called()
        self.assertEqual(2, len(self.rows()))

    def test_size_limit_blocks_all_without_evidence(self):
        self.file("state", {"snippet": "x" * judge.MAX_REQUEST_BYTES})
        code, result, send = self.run_ask()
        self.assertEqual(3, code)
        send.assert_not_called()
        self.assertTrue(all(r["error"] == "size-blocked" and r["state_ref"] is None for r in self.rows()))

    def test_ui_bands_have_no_half_cut_and_never_route(self):
        for probability, band in ((0.8, "act"), (0.79, "human"), (0.35, "human"), (0.34, "drop")):
            answer, diagnostics = judge._answer("ui.slop", {"model": "synthetic", "answers": {
                "q": {"type": "noul", "noul": probability}}}, "q", self.spec)
            self.assertEqual({"result": band}, answer)
            self.assertEqual("ui-slop-initial-bands", diagnostics["threshold_id"])
