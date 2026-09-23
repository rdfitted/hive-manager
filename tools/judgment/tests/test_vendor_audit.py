import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import audit  # noqa: E402
import verify_vendor  # noqa: E402


class VendoredAuditTests(unittest.TestCase):
    def test_all_four_vendored_files_verify(self):
        self.assertEqual(
            [
                "audit.py: OK",
                "calibration.py: OK",
                "ledger-schema.json: OK",
                "ledger.py: OK",
            ],
            verify_vendor.verify(JUDGMENT_ROOT),
        )

    def test_either_new_vendor_file_tampering_fails_closed(self):
        manifest = json.loads(
            (JUDGMENT_ROOT / "vendor-manifest.json").read_text(encoding="utf-8")
        )
        for tampered_name in ("audit.py", "calibration.py"):
            with self.subTest(file=tampered_name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                shutil.copy2(JUDGMENT_ROOT / "vendor-manifest.json", root)
                for name in manifest["files"]:
                    shutil.copy2(JUDGMENT_ROOT / name, root)
                with (root / tampered_name).open("ab") as handle:
                    handle.write(b"\n# tampered\n")
                with self.assertRaisesRegex(ValueError, "normalized content differs"):
                    verify_vendor.verify(root)

    def test_heldout_split_is_stable(self):
        self.assertFalse(audit.is_heldout("0" * 64))
        self.assertTrue(audit.is_heldout("0" * 63 + "1"))


if __name__ == "__main__":
    unittest.main()
