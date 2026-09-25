import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import retrieval_replay  # noqa: E402


FIXTURE = Path(__file__).parent / "fixtures" / "match-type-basic"
SESSION_ID = "44444444-4444-4444-8444-444444444444"
AGENT_ID = f"{SESSION_ID}-worker-1"


class MatchTypePrecisionTests(unittest.TestCase):
    def test_persisted_edge_rationale_groups_ack_precision(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            session = root / "synthetic-repo" / ".hive-manager" / SESSION_ID
            store = root / "store"
            graph = store / SESSION_ID / "state" / "work-graph.json"
            graph.parent.mkdir(parents=True)
            graph.write_bytes((FIXTURE / "work-graph.json").read_bytes())
            prompt = session / "prompts" / f"{AGENT_ID}-context.json"
            prompt.parent.mkdir(parents=True)
            prompt.write_bytes((FIXTURE / "spawn-context.json").read_bytes())
            (session / "plan.md").write_text(
                "# Synthetic legacy plan\n\nReview knowledge references.\n",
                encoding="utf-8",
            )
            acks = store / SESSION_ID / "state" / "knowledge-acks.jsonl"
            acks.write_text(
                json.dumps({
                    "schema_version": "hive.knowledge-ack/v1",
                    "session_id": SESSION_ID,
                    "agent_id": AGENT_ID,
                    "knowledge_ack": ["k1", "k3", "k5"],
                    "recorded_at": "2026-09-24T00:00:00Z",
                }) + "\n", encoding="utf-8"
            )
            with patch.object(
                retrieval_replay, "tracked_files", return_value=([], None)
            ):
                report = retrieval_replay.run_replay(
                    [session.parent],
                    ledger_path=root / "ledger.jsonl",
                    stale_report=root / "stale.json",
                    session_store_root=store,
                )
            self.assertEqual([], report["spawn_context_errors"])
            self.assertEqual([], report["knowledge_ack_errors"])
            precision = report["knowledge_ack_metrics"]["precision"]
            self.assertEqual(
                precision,
                report["repositories"]["synthetic-repo"]
                ["knowledge_ack_metrics"]["precision"],
            )
            self.assertEqual(3, precision["overall"]["used"])
            self.assertEqual(5, precision["overall"]["shown"])
            self.assertEqual(
                {
                    "inferred-scope:exact": (1, 2, 0.5),
                    "inferred-scope:suffix": (1, 2, 0.5),
                    "unknown": (1, 1, 1.0),
                },
                {
                    key: (value["used"], value["shown"], value["rate"])
                    for key, value in precision["by_match_type"].items()
                },
            )
            self.assertEqual(
                {"project": (3, 3, 1.0), "institutional": (0, 2, 0.0)},
                {
                    key: (value["used"], value["shown"], value["rate"])
                    for key, value in precision["by_provenance"].items()
                },
            )


if __name__ == "__main__":
    unittest.main()
