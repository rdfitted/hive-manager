import json
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

from retrieval_rules import RUST_RULESET, run_ruleset  # noqa: E402


FIXTURES_ROOT = JUDGMENT_ROOT / "fixtures"
GOLDEN_PATH = JUDGMENT_ROOT / "golden" / "retrieval-golden.json"


def _materialize(source: Path, destination: Path) -> Path:
    root = destination / source.name
    root.mkdir()
    for path in source.rglob("*"):
        relative = path.relative_to(source)
        if relative.parts[0] == "ai-docs":
            relative = Path(".ai-docs", *relative.parts[1:])
        target = root / relative
        if path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        content = path.read_text(encoding="utf-8").replace("\r\n", "\n").replace("\r", "\n")
        if relative.name == "codegraph.json":
            artifact = json.loads(content)
            self_root = artifact.get("root")
            if self_root != "<ROOT>":
                raise AssertionError("fixture codegraph root must use <ROOT>")
            artifact["root"] = str(root.resolve())
            content = json.dumps(artifact, indent=2)
        target.write_text(content, encoding="utf-8", newline="\n")
    return root


def _normalize_root(value, root: Path):
    if isinstance(value, dict):
        return {key: _normalize_root(item, root) for key, item in value.items()}
    if isinstance(value, list):
        return [_normalize_root(item, root) for item in value]
    if isinstance(value, str):
        return value.replace(str(root.resolve()), "<ROOT>")
    return value


class RetrievalParityTests(unittest.TestCase):
    def test_current_rules_match_rust_golden(self):
        self.assertEqual("current", RUST_RULESET)
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
                root = _materialize(fixture, temporary_root)
                result = _normalize_root(run_ruleset(RUST_RULESET, root), root)
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
