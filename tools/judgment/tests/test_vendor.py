import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

import verify_vendor  # noqa: E402


class VendorVerificationTests(unittest.TestCase):
    def _copy_vendor(self, destination: Path) -> None:
        manifest = json.loads(
            (JUDGMENT_ROOT / "vendor-manifest.json").read_text(encoding="utf-8")
        )
        shutil.copy2(JUDGMENT_ROOT / "vendor-manifest.json", destination)
        for name in manifest["files"]:
            shutil.copy2(JUDGMENT_ROOT / name, destination)

    def test_checked_in_vendor_is_verified(self):
        self.assertEqual(
            [
                "audit.py: OK",
                "calibration.py: OK",
                "ledger-schema.json: OK",
                "ledger.py: OK",
            ],
            verify_vendor.verify(JUDGMENT_ROOT),
        )

    def test_tampered_vendor_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self._copy_vendor(root)
            with (root / "ledger.py").open("a", encoding="utf-8") as handle:
                handle.write("# tampered\n")
            with self.assertRaisesRegex(ValueError, "content differs"):
                verify_vendor.verify(root)

    def test_unlisted_vendor_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self._copy_vendor(root)
            (root / "unlisted.py").write_text(
                "# Vendored source SHA-256 " + "0" * 64 + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "unmanifested vendored files"):
                verify_vendor.verify(root)


if __name__ == "__main__":
    unittest.main()
