import ast
import sys
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
FORBIDDEN_IMPORTS = {"http", "requests", "socket", "urllib"}


class OfflineSafetyTests(unittest.TestCase):
    def test_no_judgment_module_imports_network_packages(self):
        violations = []
        network_importers = set()
        transport_nonstdlib = []
        for path in sorted(JUDGMENT_ROOT.rglob("*.py")):
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            for node in ast.walk(tree):
                imports = []
                if isinstance(node, ast.Import):
                    imports = [(alias.name, None) for alias in node.names]
                elif isinstance(node, ast.ImportFrom) and node.module:
                    imports = [(node.module, alias.name) for alias in node.names]
                for name, imported in imports:
                    relative = path.relative_to(JUDGMENT_ROOT).as_posix()
                    if relative == "jev_transport.py" and name.split(".", 1)[0] \
                            not in sys.stdlib_module_names | {"__future__"}:
                        transport_nonstdlib.append(name)
                    if name.split(".", 1)[0] in FORBIDDEN_IMPORTS:
                        network_importers.add(relative)
                        transport_import = (
                            relative == "jev_transport.py"
                            and (name in {"urllib.request", "urllib.error"}
                                 or (name == "urllib" and imported in {"request", "error"}))
                        )
                        if not transport_import:
                            violations.append(f"{relative}: {name}")
        self.assertEqual([], violations)
        self.assertEqual({"jev_transport.py"}, network_importers)
        self.assertEqual([], transport_nonstdlib)


if __name__ == "__main__":
    unittest.main()
