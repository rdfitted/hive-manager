"""The smoke must fail closed: a check it cannot evaluate is never a pass."""
import base64
import contextlib
import importlib.util
import io
import json
import os
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).with_name("smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

BASE = "https://github.com/owner/repo"
KEY_ID = bytes(range(8))


def encoded(text):
    return base64.b64encode(text.encode()).decode()


def manifest(tag="v1.2.3", **changes):
    entry = {"url": f"{BASE}/releases/download/{tag}/App.tar.gz", "signature": "x"}
    platforms = {name: dict(entry) for name in smoke.PLATFORMS}
    platforms["windows-x86_64"]["url"] = f"{BASE}/releases/download/{tag}/App-setup.exe"
    return {"version": tag[1:], "platforms": platforms, **changes}


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


class MissingEnvTests(unittest.TestCase):
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


class ManifestShapeTests(unittest.TestCase):
    def test_accepts_the_release_shape_and_returns_unique_urls(self):
        self.assertEqual(smoke.check_manifest_shape(manifest(), BASE, "v1.2.3"), [
            f"{BASE}/releases/download/v1.2.3/App-setup.exe",
            f"{BASE}/releases/download/v1.2.3/App.tar.gz"])

    def test_rejects_drift(self):
        other_host = manifest()
        other_host["platforms"]["windows-x86_64"]["url"] = "https://evil.example/App-setup.exe"
        traversal = manifest()
        traversal["platforms"]["windows-x86_64"]["url"] = f"{BASE}/releases/download/v1.2.3/../x.exe"
        missing = manifest()
        del missing["platforms"]["darwin-x86_64"]
        for bad in (manifest(version="9.9.9"), other_host, traversal, missing, [], {"version": "1.2.3"}):
            with self.assertRaises(smoke.Failed):
                smoke.check_manifest_shape(bad, BASE, "v1.2.3")


class KeyTests(unittest.TestCase):
    def test_reads_matching_key_ids_from_pubkey_and_signature(self):
        public = encoded("untrusted comment: minisign public key\n"
                         + base64.b64encode(b"Ed" + KEY_ID + bytes(32)).decode() + "\n")
        signature = encoded("untrusted comment: signature\n"
                            + base64.b64encode(b"ED" + KEY_ID + bytes(64)).decode()
                            + "\ntrusted comment: file\n" + base64.b64encode(bytes(64)).decode() + "\n")
        self.assertEqual(smoke.public_key_id(public), "0706050403020100")
        self.assertEqual(smoke.signature_key_id(signature), "0706050403020100")

    def test_rejects_malformed_keys(self):
        for bad in ("", "not base64!", encoded("one line"), encoded("a\n" + base64.b64encode(b"short").decode())):
            with self.assertRaises(smoke.Failed):
                smoke.public_key_id(bad)
            with self.assertRaises(smoke.Failed):
                smoke.signature_key_id(bad)

    def test_committed_config_has_a_readable_pubkey(self):
        _, updater, key_id = smoke.read_updater_config(smoke.ROOT)
        self.assertRegex(key_id, r"^[0-9A-F]{16}$")
        self.assertEqual(updater["endpoints"], [f"{smoke.RELEASES_URL}/releases/latest/download/latest.json"])


class RefAdvertisementTests(unittest.TestCase):
    def test_reads_plain_and_peeled_tags(self):
        def packet(text):
            return f"{len(text) + 4:04x}{text}".encode()
        body = (packet("# service=git-upload-pack\n") + b"0000"
                + packet("a" * 40 + " HEAD\0multi_ack side-band\n")
                + packet("b" * 40 + " refs/tags/v1.2.3\n")
                + packet("c" * 40 + " refs/tags/v1.2.3^{}\n") + b"0000")
        refs = smoke.parse_ref_advertisement(body)
        self.assertEqual(refs["HEAD"], "a" * 40)
        self.assertEqual(refs["refs/tags/v1.2.3"], "b" * 40)
        self.assertEqual(refs["refs/tags/v1.2.3^{}"], "c" * 40)


if __name__ == "__main__":
    unittest.main()
