"""Fixture tests for slop-check. Runs the real pinned engine.

    python -I -m unittest discover -s workflow-pack/skills/slop-check/tests
"""
import json
import os
import re
import shutil
import sys
import tempfile
import unittest
from unittest import mock
import subprocess
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


class FileUrlRoot(unittest.TestCase):
    """A file:// scan with no --root resolves its root from the page, not from the caller's cwd."""

    def test_file_url_outside_cwd_scans_without_explicit_root(self):
        d = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        shutil.copy(FIX / "side_tab.html", d / "side_tab.html")
        (d / slop_check.IGNORE_FILE).write_text(json.dumps({"ignores": []}), encoding="utf-8")
        # Run from a directory that does not contain the page, wherever the suite was started.
        elsewhere = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, elsewhere, ignore_errors=True)
        self.addCleanup(os.chdir, Path.cwd())
        os.chdir(elsewhere)
        code, v = run((d / "side_tab.html").as_uri())
        self.assertEqual((code, v["errors"]), (2, []), json.dumps(v, indent=2))
        self.assertEqual({f["relpath"] for f in v["gating"]}, {"side_tab.html"})
        self.assertIn(("side-tab", "ban-side-tab"), rules(v, "gating"))


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


class EngineLaunchRetry(unittest.TestCase):
    TIMEOUT = b"Timed out after 30000 ms while waiting for the WS endpoint URL to appear in stdout!"

    def invoke(self, responses):
        with mock.patch.object(slop_check.subprocess, "run", side_effect=responses) as engine:
            result = slop_check.run_engine(Path("fake-engine"), "fixture.html", None, False)
        return result, engine

    def output(self, code, stdout=b"[]", stderr=b""):
        return subprocess.CompletedProcess([], code, stdout, stderr)

    def test_timeout_then_success(self):
        result, engine = self.invoke([self.output(1, b"", self.TIMEOUT), self.output(0)])
        self.assertEqual(result, (0, [], "", 1))
        self.assertEqual(engine.call_count, 2)

    def test_timeout_twice(self):
        result, engine = self.invoke([self.output(1, b"", self.TIMEOUT)] * 2)
        self.assertEqual((result[0], result[3], engine.call_count), (1, 1, 2))

    def test_exit_two_never_retries(self):
        result, engine = self.invoke([self.output(2, b"[]", self.TIMEOUT)])
        self.assertEqual((result[0], result[3], engine.call_count), (2, 0, 1))

    def test_exit_zero_never_retries(self):
        result, engine = self.invoke([self.output(0, b"[]", self.TIMEOUT)])
        self.assertEqual((result[0], result[3], engine.call_count), (0, 0, 1))

    def test_findings_never_retry(self):
        result, engine = self.invoke([self.output(1, b'[{"antipattern":"fixture"}]', self.TIMEOUT)])
        self.assertEqual((result[3], engine.call_count), (0, 1))
        self.assertEqual(result[1], [{"antipattern": "fixture"}])

    def test_other_errors_never_retry(self):
        result, engine = self.invoke([self.output(1, b"[]", b"scan failed")])
        self.assertEqual((result[3], engine.call_count), (0, 1))

    def test_windows_launch_is_hidden(self):
        with mock.patch.object(slop_check.sys, "platform", "win32"), \
                mock.patch.object(slop_check.subprocess, "CREATE_NO_WINDOW", 0x08000000, create=True):
            _, engine = self.invoke([self.output(0)])
        self.assertEqual(engine.call_args.kwargs["creationflags"], 0x08000000)

    def test_retry_launch_exception_keeps_count_in_verdict(self):
        for failure in (OSError("synthetic retry launch failure"),
                        subprocess.TimeoutExpired("fake-engine", 300)):
            with self.subTest(failure=type(failure).__name__), \
                    mock.patch.object(slop_check, "ensure_engine", return_value=Path("fake-engine")), \
                    mock.patch.object(slop_check.subprocess, "run", side_effect=[
                        self.output(1, b"", self.TIMEOUT), failure]) as engine:
                code, verdict = run(str(FIX / "clean.html"), "--root", str(FIX))
            self.assertEqual(code, 1)
            self.assertEqual(engine.call_count, 2)
            self.assertEqual(verdict["targets"][0]["retry_count"], 1)
            self.assertIsNone(verdict["targets"][0]["engine_exit"])
            self.assertEqual(verdict["verdict"], "error")

    def test_retry_invalid_json_keeps_count_in_verdict(self):
        with mock.patch.object(slop_check, "ensure_engine", return_value=Path("fake-engine")), \
                mock.patch.object(slop_check.subprocess, "run", side_effect=[
                    self.output(1, b"", self.TIMEOUT), self.output(1, b"invalid")]) as engine:
            code, verdict = run(str(FIX / "clean.html"), "--root", str(FIX))
        self.assertEqual((code, engine.call_count, verdict["targets"][0]["retry_count"]), (1, 2, 1))
        self.assertIn("engine returned invalid JSON", verdict["errors"][0])


if __name__ == "__main__":
    unittest.main()
