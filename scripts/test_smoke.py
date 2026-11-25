"""The smoke must fail closed: a check it cannot evaluate is never a pass."""
import base64
import contextlib
import importlib.util
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).with_name("smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

BASE = "https://github.com/owner/repo"
TAG = "v1.2.3"


def encoded(text):
    return base64.b64encode(text.encode()).decode()


def manifest(**changes):
    entry = {"url": f"{BASE}/releases/download/{TAG}/App.tar.gz", "signature": "x"}
    platforms = {name: dict(entry) for name in smoke.PLATFORMS}
    platforms["windows-x86_64"]["url"] = f"{BASE}/releases/download/{TAG}/App-setup.exe"
    return {"version": TAG[1:], "platforms": platforms, **changes}


def release(**changes):
    names = {"App.tar.gz": 30, "App.tar.gz.sig": 4, "App-setup.exe": 20, "App-setup.exe.sig": 4,
             "App.dmg": 25, "latest.json": 9}
    assets = [{"name": name, "size": size, "state": "uploaded"} for name, size in names.items()]
    return {"tag_name": TAG, "draft": False, "prerelease": False, "assets": assets, **changes}


SIZES = {"App.tar.gz": 30, "App.tar.gz.sig": 4, "App-setup.exe": 20, "App-setup.exe.sig": 4, "latest.json": 9}


def packet(text):
    return f"{len(text) + 4:04x}{text}".encode()


def run_main(*argv):
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
        code = smoke.main(list(argv))
    return code, json.loads(out.getvalue().strip().splitlines()[-1])


class ExitCodeTests(unittest.TestCase):
    def test_failure_outranks_could_not_run_and_both_outrank_pass(self):
        ok = {"name": "a", "ok": True}
        blocked = {"name": "b", "ok": False, "could_not_run": True}
        failed = {"name": "c", "ok": False}
        self.assertEqual(smoke.exit_code([ok, ok, ok]), 0)
        self.assertEqual(smoke.exit_code([ok, ok, blocked]), 2)
        self.assertEqual(smoke.exit_code([ok, blocked, failed]), 1)

    def test_fewer_than_three_checks_is_not_a_pass(self):
        self.assertEqual(smoke.exit_code([{"name": "a", "ok": True}]), 2)

    def test_a_crashing_check_is_a_failure(self):
        def crash():
            raise RuntimeError("boom")
        results = smoke.run_checks([("crash", crash)], smoke.time.monotonic() + 60)
        self.assertFalse(results[0]["ok"])
        self.assertNotIn("could_not_run", results[0])

    def test_an_exhausted_budget_blocks_instead_of_passing(self):
        results = smoke.run_checks([("late", lambda: "ran")], smoke.time.monotonic() - 1)
        self.assertTrue(results[0]["could_not_run"])

    def test_an_unknown_sha_or_tag_is_never_a_match(self):
        with self.assertRaises(smoke.CouldNotRun):
            smoke.expectation({}, "sha", "a" * 40)()
        with self.assertRaises(smoke.Failed):
            smoke.expectation({"tag": "v1.2.2"}, "tag", TAG)()
        self.assertEqual(smoke.expectation({"tag": TAG}, "tag", TAG)(), TAG)


    def test_expectations_run_right_after_the_value_is_known(self):
        checks = [("a", None), ("checkout-commit", None), ("slow", None)]
        smoke.place_after(checks, ("checkout-commit", "release-tag-commit"), ("sha-matches-expected", None))
        self.assertEqual([name for name, _ in checks], ["a", "checkout-commit", "sha-matches-expected", "slow"])
        checks = [("checkout", None)]
        smoke.place_after(checks, ("latest-release-tag",), ("tag-matches-expected", None))
        self.assertEqual([name for name, _ in checks], ["checkout", "tag-matches-expected"])


    def test_children_never_inherit_the_api_token(self):
        seen = {}

        class Finished:
            returncode = 0

            def communicate(self, timeout=None):
                return b"", None

        def popen(args, **options):
            seen.update(options["env"])
            return Finished()
        with patch.dict(os.environ, {"GITHUB_TOKEN": "t1", "GH_TOKEN": "t2", "SMOKE_KEEP": "yes"}), \
                patch.object(smoke.subprocess, "Popen", side_effect=popen):
            smoke.run_tool(["git", "--version"], smoke.ROOT, smoke.time.monotonic() + 30)
        self.assertEqual(seen.get("SMOKE_KEEP"), "yes")
        self.assertNotIn("GITHUB_TOKEN", seen)
        self.assertNotIn("GH_TOKEN", seen)


class CouldNotRunTests(unittest.TestCase):
    def test_missing_env_exits_two_and_prints_the_name_only(self):
        secret = "do-not-print-this-value"
        with patch.dict(smoke.REQUIRED_ENV, {"staging": ("SMOKE_TEST_ABSENT", "SMOKE_TEST_PRESENT")}), \
                patch.dict(os.environ, {"SMOKE_TEST_PRESENT": secret}), \
                patch.object(smoke, "staging_checks", side_effect=lambda *_: [("required-env", smoke.required_env("staging"))]):
            os.environ.pop("SMOKE_TEST_ABSENT", None)
            code, report = run_main("--target", "staging", "--base-url", str(smoke.ROOT))
        self.assertEqual(code, 2)
        self.assertEqual(report["checks"][0]["detail"], "missing env: SMOKE_TEST_ABSENT")
        self.assertNotIn(secret, json.dumps(report))

    def test_staging_without_a_checkout_exits_two(self):
        code, report = run_main("--target", "staging", "--base-url", "https://example.invalid")
        self.assertEqual(code, 2)
        self.assertEqual(report["target"], "staging")
        self.assertIsNone(report["sha"])

    def test_staging_refuses_a_tree_that_is_not_a_clean_commit(self):
        with tempfile.TemporaryDirectory() as temp:
            checks = dict(smoke.staging_checks(Path(temp), smoke.time.monotonic() + 30, {}, Path(temp)))
            with self.assertRaises(smoke.CouldNotRun):
                checks["checkout-commit"]()

        def dirty(args, cwd, deadline):
            return (0, "a" * 40 + "\n") if args[1] == "rev-parse" else (0, " M file\n")
        state = {}
        with patch.object(smoke, "run_tool", side_effect=dirty):
            checks = dict(smoke.staging_checks(smoke.ROOT, smoke.time.monotonic() + 30, state, smoke.ROOT))
            with self.assertRaises(smoke.CouldNotRun):
                checks["checkout-commit"]()
        self.assertNotIn("sha", state)


class ManifestShapeTests(unittest.TestCase):
    def test_accepts_the_release_shape_and_returns_unique_urls(self):
        self.assertEqual(smoke.check_manifest_shape(manifest(), BASE, TAG), [
            f"{BASE}/releases/download/{TAG}/App-setup.exe",
            f"{BASE}/releases/download/{TAG}/App.tar.gz"])

    def test_rejects_drift(self):
        def with_windows_url(url):
            changed = manifest()
            changed["platforms"]["windows-x86_64"]["url"] = url
            return changed
        missing = manifest()
        del missing["platforms"]["darwin-x86_64"]
        prefix = f"{BASE}/releases/download/{TAG}/"
        for bad in (manifest(version="9.9.9"), missing, [], {"version": TAG[1:]},
                    with_windows_url("https://evil.example/App-setup.exe"),
                    with_windows_url(prefix + "../x.exe"), with_windows_url(prefix + ".."),
                    with_windows_url(prefix + "latest.json")):
            with self.assertRaises(smoke.Failed):
                smoke.check_manifest_shape(bad, BASE, TAG)


class InventoryTests(unittest.TestCase):
    def test_accepts_a_complete_published_release(self):
        self.assertIn("App.dmg", smoke.check_inventory(release(), TAG, SIZES))

    def test_rejects_an_incomplete_or_unpublished_release(self):
        def without(name):
            return release(assets=[a for a in release()["assets"] if a["name"] != name])

        def with_asset(name, **changes):
            return release(assets=[dict(a, **changes) if a["name"] == name else a for a in release()["assets"]])
        for bad in (release(draft=True), release(prerelease=True), release(tag_name="v1.2.2"), None,
                    without("App.dmg"), with_asset("App.dmg", size=0), with_asset("App.dmg", state="starter"),
                    without("App-setup.exe.sig"), without("latest.json"), with_asset("App.tar.gz", size=29),
                    release(assets=release()["assets"] + [{"name": "Old.dmg", "size": 1, "state": "uploaded"}])):
            with self.assertRaises(smoke.Failed):
                smoke.check_inventory(bad, TAG, SIZES)


class KeyTests(unittest.TestCase):
    def test_prints_the_key_id_the_way_minisign_does(self):
        public = encoded("untrusted comment: minisign public key\n"
                         + base64.b64encode(b"Ed" + bytes(range(8)) + bytes(32)).decode() + "\n")
        self.assertEqual(smoke.public_key_id(public), "0706050403020100")
        crlf = encoded("untrusted comment: minisign public key\r\n"
                       + base64.b64encode(b"Ed" + bytes(range(8)) + bytes(32)).decode() + "\r\n")
        self.assertEqual(smoke.public_key_id(crlf), "0706050403020100")

    def test_rejects_malformed_keys(self):
        key = base64.b64encode(b"Ed" + bytes(range(8)) + bytes(32)).decode()
        # The shipped updater reads line two untrimmed: a leading blank line, a bare key
        # and non-UTF-8 bytes all break updates, so none may pass here.
        for bad in ("", None, "not base64!", encoded("one line"), encoded("a\n" + base64.b64encode(b"short").decode()),
                    encoded("\nuntrusted comment: x\n" + key + "\n"), key, encoded("comment\n " + key),
                    encoded("comment\n" + key + "\r"), encoded("comment\n" + key) + "=",
                    encoded("comment\n" + key + "="), encoded("comment\r" + key + "\n"),
                    base64.b64encode(b"\xff\xfe\n" + key.encode()).decode()):
            with self.assertRaises(smoke.Failed):
                smoke.public_key_id(bad)

    def test_committed_config_points_at_this_repository(self):
        _, updater, key_id = smoke.read_updater_config(smoke.ROOT)
        self.assertRegex(key_id, r"^[0-9A-F]{16}$")
        self.assertEqual(updater["endpoints"], [f"{smoke.RELEASES_URL}/releases/latest/download/latest.json"])


class RefAdvertisementTests(unittest.TestCase):
    BODY = (packet("# service=git-upload-pack\n") + b"0000"
            + packet("a" * 40 + " HEAD\0multi_ack side-band\n")
            + packet("b" * 40 + " refs/tags/v1.2.3\n")
            + packet("c" * 40 + " refs/tags/v1.2.3^{}\n") + b"0000")

    def test_reads_plain_and_peeled_tags(self):
        refs = smoke.parse_ref_advertisement(self.BODY)
        self.assertEqual(refs["HEAD"], "a" * 40)
        self.assertEqual(refs["refs/tags/v1.2.3"], "b" * 40)
        self.assertEqual(refs["refs/tags/v1.2.3^{}"], "c" * 40)

    def test_rejects_a_truncated_or_malformed_body(self):
        for bad in (self.BODY[:-4], self.BODY[:-3], self.BODY[:-1], self.BODY[:-10], self.BODY[:60],
                    self.BODY + b"00", b"0", b"zzzz", b"0003"):
            with self.assertRaises(ValueError):
                smoke.parse_ref_advertisement(bad)


class HttpTests(unittest.TestCase):
    def test_refuses_plain_http_and_a_spent_budget(self):
        with self.assertRaises(smoke.Failed):
            smoke.http_get("http://github.com/owner/repo", smoke.time.monotonic() + 30)
        with self.assertRaises(smoke.CouldNotRun):
            smoke.http_get(BASE, smoke.time.monotonic() - 1)

    def test_a_redirect_away_from_https_is_refused(self):
        handler = smoke._HttpsRedirect()
        request = smoke.urllib.request.Request(BASE)
        with self.assertRaises(smoke.Failed):
            handler.redirect_request(request, None, 302, "Found", {}, "http://example.com/x")


if __name__ == "__main__":
    unittest.main()
