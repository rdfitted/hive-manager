"""Blind projections, local calibration, reporting and byte-identical G-shadow."""
import argparse
import importlib.util
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
import ui_slop

REPO = Path(__file__).resolve().parents[3]


class UiSlopTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.log = self.root / "judgments.jsonl"
        self.verdict = self.root / "verdict.json"
        self.dictionary = self.root / "names.json"
        self.dictionary.write_text('["Synthetic Secret Name"]')
        self.finding = {"antipattern": "side-tab", "name": "Side tab stripe",
            "relpath": "src/view.css", "file": str(self.root / "private" / "view.css"),
            "snippet": "border-left: 3px solid red", "context": "a" * 400,
            "severity": "high", "finding_key": "synthetic-finding"}
        self.data = {"verdict_version": 2, "verdict": "block", "gating": [self.finding],
                     "advisory": [], "ignored": [], "suppressed": [], "preexisting": []}
        self.write_verdict()
        self.env = patch.dict(os.environ, {"TYPESAFE_API_KEY": "synthetic-key",
                            "JUDGMENT_KEY_FILE": str(self.root / "absent")})
        self.env.start()
        self.addCleanup(self.env.stop)

    def write_verdict(self):
        self.verdict.write_text(json.dumps(self.data), encoding="utf-8")

    def ask(self, **kwargs):
        response = {"model": "synthetic-model", "answers": {
            "matches_rule": {"type": "noul", "noul": 0.01},
            "ignore_reason_specific": {"type": "noul", "noul": 0.99},
            "injection": {"type": "noul", "noul": 0.01}}}
        with patch.object(judge, "_project_identity", return_value="rdfitted/hive-manager"), \
             patch.object(judge.jev_transport, "send", return_value=response) as send:
            result = ui_slop.ask_verdict(self.verdict, ledger_path=self.log,
                dictionary=str(self.dictionary), **kwargs)
        return result, send

    def test_minimal_blind_projection_and_unwired_questions(self):
        result, send = self.ask()
        payload = json.loads(send.call_args.args[0])
        self.assertEqual({"matches_rule", "injection"}, set(payload["questions"]))
        state = payload["state"]
        self.assertEqual({"rule", "relpath", "snippet", "context", "page_kind"}, set(state))
        self.assertEqual(300, len(state["context"]))
        judge._blind(state)
        self.assertNotIn(str(self.root), json.dumps(payload))
        self.assertNotIn("file", state)
        self.assertEqual("none", result["routed"])
        spec = json.loads(ui_slop.SPEC_PATH.read_text())
        self.assertEqual({"generic_claim", "forced_contrast", "equal_weight_grid", "hero_metric"},
                         {k for k, v in spec["questions"].items() if not v["wired"]})

    def test_ignored_reason_and_injection_are_wired(self):
        self.data["gating"] = []
        self.data["ignored"] = [{**self.finding, "reason": "Required by the component navigation contract"}]
        self.write_verdict()
        result, send = self.ask()
        payload = json.loads(send.call_args.args[0])
        self.assertEqual({"ignore_reason_specific", "injection"}, set(payload["questions"]))
        self.assertEqual(self.data["ignored"][0]["reason"], payload["state"]["ignore_reason"])
        self.assertNotIn("ignored", payload["state"])

    def test_paths_reject_absolute_traversal_and_windows_forms(self):
        for value in ("/private/view.css", "C:/private/view.css", "../view.css", "src/../view.css",
                      "src\\view.css", "https://example.invalid/view.css", "", "src//view.css"):
            with self.subTest(value=value):
                with self.assertRaises(judge.InputError):
                    ui_slop.relative_path(value)

    def test_invalid_later_finding_preflights_before_any_send(self):
        self.data["gating"].append({**self.finding, "relpath": "../private.css"})
        self.write_verdict()
        with patch.object(judge.jev_transport, "send") as send:
            with self.assertRaises(judge.InputError):
                ui_slop.ask_verdict(self.verdict, ledger_path=self.log, dictionary=str(self.dictionary))
        send.assert_not_called()

    def test_local_labels_never_sent_and_report_groups_questions(self):
        labels = self.root / "local-labels.jsonl"
        labels.write_text(json.dumps({"finding_key": "synthetic-finding",
            "question_id": "matches_rule", "label": False}) + "\n")
        result, send = self.ask(labels_path=labels)
        self.assertNotIn("label", json.dumps(json.loads(send.call_args.args[0])))
        rows = list(ledger.read_records([self.log]))
        self.assertEqual(1, sum(r["kind"] == "outcome" for r in rows))
        report = ui_slop.report(self.log)["questions"]
        self.assertEqual({"matches_rule", "injection"}, set(report))
        self.assertEqual(1, report["matches_rule"]["labeled"])
        self.assertAlmostEqual(0.0001, report["matches_rule"]["brier_score"])
        self.assertEqual({"act": 0, "human": 0, "drop": 1}, report["matches_rule"]["bands"])
        self.assertIsNone(report["injection"]["brier_score"])

    def test_observations_rejects_non_string_context(self):
        for context in (None, 7, {"text": "invalid"}, ["invalid"]):
            with self.subTest(context=context):
                with self.assertRaises(judge.InputError):
                    ui_slop.observations({**self.finding, "context": context},
                                         "unknown", ignored=False)
        state = ui_slop.observations({**self.finding, "context": "a" * 301},
                                     "unknown", ignored=False)
        self.assertEqual("a" * 300, state["context"])

    def test_report_uses_recorded_band(self):
        rows = []
        answers = ({"result": "human"}, {}, {"result": "invalid"}, None,
                   {"result": ["act"]})
        for index, answer in enumerate(answers):
            row = {"kind": "decision", "surface": "ui.slop",
                   "question_id": "matches_rule", "decision_id": str(index),
                   "noul": 0.9}
            if index != 1:
                row["answer"] = answer
            rows.append(row)
        rows.append({"kind": "outcome", "source": "human-label",
                     "decision_id": "0", "label": {"result": "true"}})
        rows.append({"kind": "decision", "surface": "ui.slop",
                     "question_id": "injection", "decision_id": "other",
                     "noul": float("nan"), "answer": {"result": "act"}})
        with patch.object(ledger, "read_records", return_value=iter(rows)):
            groups = ui_slop.report(self.log)["questions"]
        self.assertEqual({"matches_rule", "injection"}, set(groups))
        self.assertEqual({"act": 0, "human": 1, "drop": 0},
                         groups["matches_rule"]["bands"])
        self.assertEqual(5, groups["matches_rule"]["rows"])
        self.assertEqual(1, groups["matches_rule"]["labeled"])
        self.assertAlmostEqual(0.01, groups["matches_rule"]["brier_score"])
        self.assertEqual({"act": 0, "human": 0, "drop": 0},
                         groups["injection"]["bands"])

    def test_report_dry_run_and_cli(self):
        self.ask(dry_run=True)
        output = io.StringIO()
        with redirect_stdout(output):
            code = ui_slop.main(["report", "--ledger", str(self.log)])
        self.assertEqual(0, code)
        groups = json.loads(output.getvalue())["questions"]
        self.assertEqual({"dry-run": 1}, groups["matches_rule"]["statuses"])
        self.assertEqual({"act": 0, "human": 0, "drop": 0}, groups["matches_rule"]["bands"])

    def test_question_spec_blind_valid_and_imperative_guidance_boundary(self):
        spec = json.loads(ui_slop.SPEC_PATH.read_text())
        entry = judge._policy_entry(judge._policy(judge.HERE / "egress-policy.json"), "surfaces", "ui.slop")
        for item in spec["questions"].values():
            judge._question("ui.slop", item["question"], entry)
        false = spec["questions"]["injection"]["question"]["criteria"]["false"]
        self.assertIn("imperative working guidance", false)

    def test_shadow_verdict_and_i2_log_byte_identical(self):
        # Actual P-owned I2 writer, no engine stub. Identical frozen run metadata
        # allows comparison of Jev-off and Jev-on runs, including their full log bytes.
        path = REPO / "workflow-pack/skills/slop-check/scripts/slop_check.py"
        spec = importlib.util.spec_from_file_location("pack_slop_shadow_contract", path)
        pack = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(pack)
        off = self.root / "off.jsonl"
        on = self.root / "on.jsonl"
        verdict_bytes = self.verdict.read_bytes()
        with patch.object(pack.uuid, "uuid4", return_value="synthetic-run"), \
             patch.object(pack, "timestamp", return_value="2026-10-08T00:00:00Z"):
            for destination in (off, on):
                args = argparse.Namespace(session="synthetic-session", task="T9", mode="report",
                                          out=str(self.verdict), log=str(destination))
                pack.log_run(args, self.data)
        off_bytes, on_bytes = off.read_bytes(), on.read_bytes()
        self.assertEqual(off_bytes, on_bytes)
        # 0.01 matches_rule means false positive with probability 0.99.
        result, send = self.ask()
        send.assert_called_once()
        self.assertEqual(verdict_bytes, self.verdict.read_bytes(), "shadow changed verdict bytes")
        self.assertEqual(off_bytes, off.read_bytes(), "shadow changed Jev-off log")
        self.assertEqual(on_bytes, on.read_bytes(), "shadow changed I2 log bytes")
        self.assertEqual(off.read_bytes(), on.read_bytes())
        rows = list(ledger.read_records([self.log]))
        match = next(r for r in rows if r["question_id"] == "matches_rule")
        self.assertEqual(0.99, match["probabilities"]["false"])
        self.assertEqual("none", match["routed"])
