"""Fixture tests for slop-check. Runs the real pinned engine.

    python -I -m unittest discover -s workflow-pack/skills/slop-check/tests
"""
import json
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SKILL = HERE.parent
FIX = SKILL / "fixtures"
sys.path.insert(0, str(SKILL / "scripts"))
import slop_check  # noqa: E402


def run(*args: str) -> tuple[int, dict]:
    with tempfile.TemporaryDirectory() as td:
        out = Path(td) / "v.json"
        code = slop_check.main([*args, "--out", str(out), "--quiet"])
        return code, json.loads(out.read_text(encoding="utf-8"))


def rules(v: dict, bucket: str) -> list[tuple[str, str]]:
    return sorted((f["antipattern"], f.get("banned") or f.get("filter") or "") for f in v[bucket])


class Fixtures(unittest.TestCase):
    """Each fixture pins one behaviour: a house ban, a false-positive filter, or a negative control."""

    def test_clean_page_passes(self):  # negative control
        code, v = run(str(FIX / "clean.html"), "--root", str(FIX))
        self.assertEqual((code, v["gating"], v["advisory"], v["suppressed"]), (0, [], [], []))

    def test_side_tab_is_banned(self):
        code, v = run(str(FIX / "side_tab.html"), "--root", str(FIX))
        self.assertEqual(code, 2)
        self.assertIn(("side-tab", "ban-side-tab"), rules(v, "gating"))

    def test_inter_is_banned(self):
        code, v = run(str(FIX / "inter.html"), "--root", str(FIX))
        self.assertEqual(code, 2)
        self.assertIn(("overused-font", "ban-inter"), rules(v, "gating"))

    def test_gradient_text_gates(self):
        code, v = run(str(FIX / "gradient_text.html"), "--root", str(FIX))
        self.assertEqual(code, 2)
        self.assertIn("gradient-text", [r for r, _ in rules(v, "gating")])

    def test_spinner_is_filtered(self):  # F1
        code, v = run(str(FIX / "spinner.tsx"), "--root", str(FIX))
        self.assertEqual(code, 0)
        self.assertEqual(rules(v, "suppressed"), [("border-accent-on-rounded", "F1-spinner")])

    def test_email_font_is_filtered(self):  # F2
        code, v = run(str(FIX / "email" / "welcome_email.ts"), "--root", str(FIX))
        self.assertEqual(code, 0)
        self.assertEqual(rules(v, "suppressed"), [("overused-font", "F2-non-ui-font")])

    def test_static_contrast_is_filtered(self):  # F3: static resolver ignores @media conditions
        code, v = run(str(FIX / "dark_tokens.html"), "--root", str(FIX))
        self.assertEqual(code, 0)
        self.assertEqual(rules(v, "suppressed"), [("low-contrast", "F3-static-contrast")])

    def test_fixture_light_paper_is_not_cream(self):
        source = (FIX / "dark_tokens.html").read_text(encoding="utf-8")
        token = re.search(r":root\{--paper:(#[0-9a-fA-F]{6});", source)
        self.assertIsNotNone(token)
        color = token.group(1)
        r, g, b = (int(color[i:i + 2], 16) for i in (1, 3, 5))
        # engine-v0.1.5 cream predicate: light, ordered RGB with warm red-blue spread.
        is_cream = min(r, g, b) >= 209 and r >= g >= b and 6 <= r - b <= 48
        self.assertFalse(is_cream, color)

    def test_rendered_scan_runs_both_viewports(self):
        url = (FIX / "dark_tokens.html").as_uri()
        code, v = run(url, "--root", str(FIX))
        self.assertEqual(code, 0, json.dumps(v, indent=2))
        self.assertEqual(sorted(t["viewport"] for t in v["targets"]), ["1280x800", "390x844"])
        self.assertEqual(v["suppressed"], [])  # rendered contrast is trusted, nothing to filter


class Ignores(unittest.TestCase):
    def repo(self, files: list[str], ignore: dict | None) -> Path:
        d = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        for f in files:
            shutil.copy(FIX / f, d / f)
        if ignore is not None:
            (d / slop_check.IGNORE_FILE).write_text(json.dumps(ignore), encoding="utf-8")
        return d

    def test_reasoned_ignore_clears_a_gate(self):
        d = self.repo(["gradient_text.html"], {"ignores": [
            {"rule": "gradient-text", "path": "*.html", "reason": "Fixture: the campaign mark is a licensed gradient wordmark"}]})
        code, v = run(str(d / "gradient_text.html"), "--root", str(d))
        # The purple/cyan fixture also trips ai-color-palette, which the ignore does not cover.
        self.assertEqual(code, 2)
        self.assertEqual({f["antipattern"] for f in v["ignored"]}, {"gradient-text"})
        self.assertEqual([f["antipattern"] for f in v["gating"]], ["ai-color-palette"])

    def test_ignore_without_reason_is_an_error(self):
        d = self.repo(["gradient_text.html"], {"ignores": [{"rule": "gradient-text", "reason": "  "}]})
        code, v = run(str(d / "gradient_text.html"), "--root", str(d))
        self.assertEqual((code, v["verdict"]), (1, "error"))

    def test_banned_rules_cannot_be_ignored(self):
        d = self.repo(["side_tab.html", "inter.html"], {"ignores": [
            {"rule": "side-tab", "reason": "we like it"},
            {"rule": "overused-font", "value": "inter", "reason": "brand font"}]})
        code, v = run(str(d), "--root", str(d))
        self.assertEqual(code, 2)
        self.assertEqual({f["banned"] for f in v["gating"]}, {"ban-side-tab", "ban-inter"})


if __name__ == "__main__":
    unittest.main()
