import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import ledger  # noqa: E402
import replay  # noqa: E402


def add_source(ledger_path: Path, source_id: str, evidence: str) -> dict:
    observations = {
        "criterion": {"number": 1, "kind": "pass_fail"},
        "evidence": evidence,
        "evidence_refs": [],
    }
    evidence_dir = ledger_path.parent / "evidence"
    evidence_dir.mkdir(exist_ok=True)
    (evidence_dir / f"{source_id}.json").write_text(
        ledger.canonical_json(observations), encoding="utf-8"
    )
    source = ledger.record_decision(
        "hive.qa.criterion",
        {"session_id": "synthetic", "criterion_number": 1},
        "incumbent-llm",
        {"result": "Fail", "rationale": "secret verdict"},
        state_hash=ledger.sha256_of(observations),
        state_ref=f"evidence/{source_id}.json",
        question_id="hive.qa.criterion",
        mode="shadow",
        decision_id=source_id,
        ledger=ledger_path,
    )
    assert source is not None
    return source


class ReplayHarnessTests(unittest.TestCase):
    def test_incumbent_named_plugin_resumes_without_replaying_its_own_rows(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "observation")
            kwargs = dict(
                plugin=lambda request, *, sampling: {"result": "Pass"},
                plugin_id="synthetic:incumbent", judge="incumbent-llm",
                runs=2, sampling={"temperature": 0}, model="synthetic-model",
            )
            self.assertEqual(2, replay.replay(ledger_path, **kwargs))
            self.assertEqual(1, len(replay.source_decisions(ledger_path)))
            self.assertEqual(0, replay.replay(ledger_path, **kwargs))
            self.assertEqual(3, len(list(ledger.read_records([ledger_path]))))
            replay_rows = [row for row in ledger.read_records([ledger_path])
                           if row.get("source_decision_id") == "source-a"]
            self.assertTrue(all(row["model"] == "synthetic-model" for row in replay_rows))
            self.assertTrue(all(row["plugin_id"] == "synthetic:incumbent" for row in replay_rows))

    def test_cli_uses_explicit_temp_ledger_and_builtin_code_judge(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "observation")
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(0, replay.main([
                    "--ledger", str(ledger_path), "--runs", "2",
                ]))
            self.assertIn("wrote 2 replay decisions", output.getvalue())
            rows = list(ledger.read_records([ledger_path]))
            replay_rows = [row for row in rows if row.get("judge") == "code"]
            self.assertEqual(2, len(replay_rows))
            self.assertTrue(all(row["answer"] == {"result": "pass"} for row in replay_rows))
            self.assertTrue(all(row["model"] == "code-test" for row in replay_rows))
            self.assertTrue(all(row["plugin_id"] == "code-test" for row in replay_rows))

    def test_default_model_and_explicit_plugin_model_share_decision_ids(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "observation")
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(0, replay.main([
                    "--ledger", str(ledger_path), "--runs", "1",
                ]))
                self.assertEqual(0, replay.main([
                    "--ledger", str(ledger_path), "--runs", "1",
                    "--model", "code-test",
                ]))
            self.assertIn("wrote 0 replay decisions", output.getvalue())
            replay_rows = [row for row in ledger.read_records([ledger_path])
                           if row.get("source_decision_id") == "source-a"]
            self.assertEqual(1, len(replay_rows))
            self.assertEqual("code-test", replay_rows[0]["model"])
            self.assertEqual(
                replay.retrieval_replay._decision_id(
                    "qa-replay", "source-a", "code-test", "code", "code-test",
                    '{"deterministic":true}', "0",
                ),
                replay_rows[0]["decision_id"],
            )

    def test_plugins_on_same_bundle_have_distinct_model_and_plugin_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "observation")
            for plugin_id in ("synthetic:one", "synthetic:two"):
                self.assertEqual(1, replay.replay(
                    ledger_path,
                    plugin=lambda request, *, sampling: {"result": "pass"},
                    plugin_id=plugin_id, judge="code", runs=1, sampling=None,
                ))
            rows = [row for row in ledger.read_records([ledger_path])
                    if row.get("source_decision_id") == "source-a"]
            self.assertEqual({"synthetic:one", "synthetic:two"},
                             {row["model"] for row in rows})
            self.assertEqual({"synthetic:one", "synthetic:two"},
                             {row["plugin_id"] for row in rows})
            self.assertTrue(all(row["model"] == row["plugin_id"] for row in rows))
            self.assertEqual({
                replay.retrieval_replay._decision_id(
                    "qa-replay", "source-a", plugin_id, "code", plugin_id, "null", "0"
                )
                for plugin_id in ("synthetic:one", "synthetic:two")
            }, {row["decision_id"] for row in rows})

    def test_replays_every_bundle_n_times_with_linked_conformant_rows(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            sources = [
                add_source(ledger_path, "source-a", "first observation"),
                add_source(ledger_path, "source-b", "second observation"),
            ]
            requests: list[bytes] = []

            def judge(request: bytes, *, sampling: dict | None):
                self.assertEqual({"deterministic": True}, sampling)
                requests.append(request)
                sampling["deterministic"] = False
                return {"result": "Pass"}

            kwargs = dict(
                plugin=judge, plugin_id="synthetic:judge", judge="code",
                runs=3, sampling={"deterministic": True},
            )
            self.assertEqual(6, replay.replay(ledger_path, **kwargs))
            self.assertEqual(0, replay.replay(ledger_path, **kwargs))
            self.assertEqual(6, len(requests))
            self.assertEqual([requests[0]] * 3, requests[:3])
            self.assertEqual([requests[3]] * 3, requests[3:])
            for source, request in zip(sources, (requests[0], requests[3])):
                self.assertEqual(
                    ledger.load_evidence(source["state_ref"], ledger_path),
                    json.loads(request),
                )
                for forbidden in ("answer", "result", "rationale", "secret verdict"):
                    self.assertNotIn(forbidden, request.decode("utf-8"))
            rows = list(ledger.read_records([ledger_path]))
            replay_rows = [row for row in rows if row.get("judge") == "code"]
            self.assertEqual(6, len(replay_rows))
            self.assertEqual(6, len({row["decision_id"] for row in replay_rows}))
            self.assertEqual({"source-a", "source-b"}, {
                row["source_decision_id"] for row in replay_rows
            })
            self.assertTrue(all(row["mode"] == "shadow" for row in replay_rows))
            self.assertTrue(all(row["sampling"] == {"deterministic": True} for row in replay_rows))
            self.assertTrue(all(row["plugin_id"] == "synthetic:judge" for row in replay_rows))
            self.assertTrue(all(row["model"] == "synthetic:judge" for row in replay_rows))
            self.assertTrue(all(row["answer"] == {"result": "Pass"} for row in replay_rows))
            self.assertEqual((8, []), ledger.validate_file([ledger_path]))

    def test_injected_verdict_fields_are_rejected_even_when_nested(self):
        safe = {"evidence": {"observation": "safe"}}
        self.assertEqual(safe, json.loads(replay.request_bytes(safe)))
        for field in ("answer", "result", "rationale"):
            with self.subTest(field=field):
                with self.assertRaisesRegex(ValueError, "verdict field"):
                    replay.request_bytes({"evidence": [{field: "leaked"}]})

    def test_frozen_evidence_hash_is_checked_before_plugin_runs(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger_path = Path(temporary) / "ledger.jsonl"
            add_source(ledger_path, "source-a", "original")
            (ledger_path.parent / "evidence" / "source-a.json").write_text(
                ledger.canonical_json({"evidence": "changed"}), encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "evidence hash mismatch"):
                replay.replay(
                    ledger_path,
                    plugin=lambda request, sampling: self.fail("plugin ran"),
                    plugin_id="synthetic:judge", judge="code", runs=1,
                    sampling={"deterministic": True},
                )


if __name__ == "__main__":
    unittest.main()
