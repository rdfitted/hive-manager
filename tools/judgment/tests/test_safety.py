import ast
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
FORBIDDEN_IMPORTS = {"http", "socket", "urllib"}


class OfflineSafetyTests(unittest.TestCase):
    def test_no_judgment_module_imports_network_packages(self):
        violations = []
        for path in sorted(JUDGMENT_ROOT.rglob("*.py")):
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            for node in ast.walk(tree):
                names = []
                if isinstance(node, ast.Import):
                    names = [alias.name for alias in node.names]
                elif isinstance(node, ast.ImportFrom) and node.module:
                    names = [node.module]
                for name in names:
                    if name.split(".", 1)[0] in FORBIDDEN_IMPORTS:
                        violations.append(f"{path.relative_to(JUDGMENT_ROOT)}: {name}")
        self.assertEqual([], violations)


if __name__ == "__main__":
    unittest.main()
