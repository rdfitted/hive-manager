"""Legacy and capture-enriched v1 spawn sidecars share one replay reader."""

import json
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import retrieval_replay  # noqa: E402


class SidecarCaptureCompatibilityTests(unittest.TestCase):
    def test_load_spawn_contexts_accepts_legacy_and_capture_fields(self):
        legacy = {
            "schema_version": "hive.spawn-context/v1",
            "session_id": "55555555-5555-4555-8555-555555555555",
            "agent_id": "synthetic-worker-1",
            "plan_task_id": "T1",
            "sampled": True,
            "kept": [
                {
                    "tag": "k1",
                    "position": 1,
                    "origin": "task",
                    "source": "project",
                    "pointer": ".ai-docs/synthetic.md",
                    "priority": 80,
                    "chars": 42,
                }
            ],
            "dropped": [],
        }
        captured = {
            **legacy,
            "agent_id": "synthetic-worker-2",
            "delivery_path": "task-bound",
            "global_summary_included": True,
            "rendered_knowledge_chars": 42,
            "decision_ids": ["synthetic-decision-id"],
            "kept": [
                {
                    **legacy["kept"][0],
                    "match_type": "inferred-scope:exact",
                    "edge_rationale": "inferred-scope:exact",
                }
            ],
        }

        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary) / "synthetic-session"
            prompts = session / "prompts"
            prompts.mkdir(parents=True)
            for name, context in (("legacy", legacy), ("captured", captured)):
                (prompts / f"{name}-context.json").write_text(
                    json.dumps(context), encoding="utf-8"
                )

            contexts, errors = retrieval_replay.load_spawn_contexts(session)

        self.assertEqual([], errors)
        self.assertEqual([captured, legacy], contexts)
        self.assertNotIn("delivery_path", contexts[1])
        self.assertNotIn("match_type", contexts[1]["kept"][0])
        self.assertEqual("task-bound", contexts[0]["delivery_path"])
        self.assertEqual(
            "inferred-scope:exact", contexts[0]["kept"][0]["edge_rationale"]
        )


if __name__ == "__main__":
    unittest.main()
