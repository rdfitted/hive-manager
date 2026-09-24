import ast
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import retrieval_placement_baseline as baseline  # noqa: E402


FIXTURE = Path(__file__).parent / "fixtures" / "placement-basic" / "session"


class PlacementBaselineTests(unittest.TestCase):
    def test_exact_aggregate_counts_and_percentiles(self):
        result = baseline.measure([FIXTURE])
        self.assertEqual(1, result["sessions"])
        self.assertEqual(3, result["spawns"])
        self.assertEqual(0, result["unreadable_sidecars"])
        self.assertEqual(
            {
                "D1_attached_refs": (2, 1.5, 2),
                "D2_global_refs": (2, 0.5, 1),
                "D2_global_summary_included": (2, 0.5, 1),
                "D3_injected_chars": (3, 100, 300),
                "D3_kept_refs": (3, 1, 2),
                "D3_dropped_refs": (3, 1, 1),
                "D4_sampled": (3, 1, 1),
                "D5_delivered_refs": (2, 1.5, 2),
                "D6_role_and_task_refs": (3, 0, 1),
                "D7_miss_sample_refs": (3, 0, 1),
                "D8_tagged_refs": (3, 0, 0),
            },
            {
                name: (metric["count"], metric["median"], metric["p90"])
                for name, metric in result["metrics"].items()
            },
        )
        self.assertNotIn("pointer", str(result))
        self.assertNotIn(".ai-docs/one.md", str(result))

    def test_read_only_and_no_network_imports(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary) / "session"
            shutil.copytree(FIXTURE, session)
            before = {
                path.relative_to(session): (path.stat().st_mtime_ns, path.read_bytes())
                for path in session.rglob("*") if path.is_file()
            }
            self.assertEqual(3, baseline.measure([session])["spawns"])
            after = {
                path.relative_to(session): (path.stat().st_mtime_ns, path.read_bytes())
                for path in session.rglob("*") if path.is_file()
            }
            self.assertEqual(before, after)
        tree = ast.parse(Path(baseline.__file__).read_text(encoding="utf-8"))
        banned = {"socket", "requests", "urllib", "http"}
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                self.assertFalse(
                    any(alias.name.split(".")[0] in banned for alias in node.names)
                )
            elif isinstance(node, ast.ImportFrom):
                self.assertNotIn((node.module or "").split(".")[0], banned)

    def test_store_root_resolves_project_prompt_folder(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stored = root / "store" / "synthetic-session"
            (stored / "state").mkdir(parents=True)
            shutil.copyfile(
                FIXTURE / "state" / "work-graph.json",
                stored / "state" / "work-graph.json",
            )
            project = root / "synthetic-project"
            prompts = project / ".hive-manager" / stored.name / "prompts"
            shutil.copytree(FIXTURE / "prompts", prompts)
            (stored / "session.json").write_text(
                json.dumps({"project_path": str(project)}), encoding="utf-8"
            )
            sessions = baseline.discover_sessions(root / "store")
            self.assertEqual([prompts.parent], sessions)
            self.assertEqual(3, baseline.measure(sessions, root / "store")["spawns"])


if __name__ == "__main__":
    unittest.main()
