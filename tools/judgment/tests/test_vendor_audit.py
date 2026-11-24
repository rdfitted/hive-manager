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


    def _passing_group(self, n_heldout, n_tune):
        hashes = [f"{i:063x}1" for i in range(n_heldout)]
        return {
            "question_type": "binary", "mode": "shadow",
            "repeat_violations": [], "rolling_agreement": None, "rolling_n": 0,
            "fail_open_rate_7d": 0.0, "fail_open_rate": 0.0,
            "n_heldout": n_heldout, "n_tune": n_tune, "agreement_heldout": 1.0,
            "heldout_hashes": hashes, "heldout_ok": {h: True for h in hashes},
            "ece_heldout": 0.0, "jitter_max": 0.0,
            "repeat_comparable": True, "repeat_groups": 3,
        }

    def test_promotion_requires_min_labeled_n(self):
        tables = json.loads((JUDGMENT_ROOT / "decision-tables.json").read_text(encoding="utf-8"))
        incumbent = {"heldout_ok": {f"{i:063x}1": True for i in range(120)}}
        # 120 held-out clears min_heldout_n (100) but 120 + 50 tune < min_labeled_n (200).
        short = audit.recommend(self._passing_group(120, 50), incumbent, tables)
        self.assertNotEqual("PROMOTE", short["action"])
        self.assertIn("labelled n 170 < 200", short["reasons"])
        enough = audit.recommend(self._passing_group(120, 80), incumbent, tables)
        self.assertEqual("PROMOTE", enough["action"], enough["reasons"])

if __name__ == "__main__":
    unittest.main()
