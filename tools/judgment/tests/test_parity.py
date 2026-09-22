import json
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

from retrieval_rules import RUST_RULESET, run_ruleset  # noqa: E402
from fixture_support import materialize_fixture  # noqa: E402


FIXTURES_ROOT = JUDGMENT_ROOT / "fixtures"
GOLDEN_PATH = JUDGMENT_ROOT / "golden" / "retrieval-golden.json"


def _normalize_root(value, root: Path):
    if isinstance(value, dict):
        return {key: _normalize_root(item, root) for key, item in value.items()}
    if isinstance(value, list):
        return [_normalize_root(item, root) for item in value]
    if isinstance(value, str):
        return value.replace(str(root.resolve()), "<ROOT>")
    return value


class RetrievalParityTests(unittest.TestCase):
    def test_rust_ruleset_matches_rust_golden(self):
        self.assertEqual("hv10+hv11", RUST_RULESET)
        self.assertTrue(GOLDEN_PATH.is_file(), "Rust golden must be checked in")
        fixture_dirs = sorted(
            path
            for path in FIXTURES_ROOT.iterdir()
            if path.is_dir() and (path / "plan.md").is_file()
        )
        self.assertGreaterEqual(len(fixture_dirs), 3, "parity fixtures must be non-vacuous")
        expected = json.loads(GOLDEN_PATH.read_text(encoding="utf-8"))
        self.assertEqual("hive-retrieval-golden/v1", expected["schema"])
        self.assertEqual([path.name for path in fixture_dirs], sorted(expected["fixtures"]))

        actual = {}
        with tempfile.TemporaryDirectory() as temporary:
            temporary_root = Path(temporary)
            for fixture in fixture_dirs:
                root = materialize_fixture(fixture.name, temporary_root)
                result = _normalize_root(run_ruleset(RUST_RULESET, root), root)
                result.pop("_task_resolution_failures", None)
                self.assertGreater(result["parsed_note_count"], 0, fixture.name)
                actual[fixture.name] = result

        declared_fixture_edges = [
            edge
            for fixture in actual.values()
            for edge in fixture["knowledge_edges"]
        ]
        self.assertTrue(declared_fixture_edges, "declared-scope fixture must emit a knowledge edge")
        self.assertEqual(expected["fixtures"], actual)


if __name__ == "__main__":
    unittest.main()
