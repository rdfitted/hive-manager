"""Local replay must not inherit the GitHub credential or host daemon socket."""
import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("local_gate", Path(__file__).with_name("check.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class ReplayIsolationTests(unittest.TestCase):
    def test_actual_replay_disables_credential_fallback_and_daemon_mount(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "scripts").mkdir()
            config = {"repository": "owner/repo", "jobs": [{"workflow": "ci.yml", "job": "test"}],
                      "hosted_only": []}
            (root / "scripts/local-gate.json").write_text(json.dumps(config), encoding="utf-8")
            (root / "ci.yml").write_text("jobs: {}", encoding="utf-8")
            pr = {"head": {"sha": "abc", "repo": {"full_name": "owner/repo"}},
                  "base": {"sha": "base", "ref": "main", "repo": {"full_name": "owner/repo"}}}
            replay = []

            def run(args, cwd=None):
                if args[:3] == ["git", "status", "--porcelain"]:
                    return b""
                if args[:3] == ["git", "rev-parse", "HEAD"]:
                    return b"abc"
                if args[:3] == ["git", "rev-parse", "--absolute-git-dir"]:
                    return str(root / ".git").encode()
                if args[:2] == ["gh", "api"]:
                    return json.dumps(pr).encode()
                if args == ["act", "--version"]:
                    return b"act test"
                if args[:2] == ["act", "pull_request"]:
                    replay.append(args)
                    return b"Job succeeded"
                raise AssertionError(args)

            with patch.object(gate, "ROOT", root), patch.object(gate, "run", side_effect=run), \
                    patch.object(gate.shutil, "which", return_value="act"), \
                    patch.object(gate, "snapshot_checkout", return_value=root), \
                    patch.object(Path, "home", return_value=root), \
                    patch.object(sys, "argv", ["check.py", "--pr", "1"]):
                gate.main()
            self.assertEqual(len(replay), 1)
            args = replay[0]
            self.assertEqual(args[args.index("--secret") + 1], "GITHUB_TOKEN=")
            self.assertEqual(args[args.index("--env") + 1], "GITHUB_TOKEN=")
            self.assertEqual(args[args.index("--container-daemon-socket") + 1], "-")


    def test_snapshot_excludes_global_ignored_files_and_local_git_config(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            source = temp / "source"
            source.mkdir()
            prefix = ["git", "-C", str(source)]
            gate.run(prefix + ["init"])
            gate.run(prefix + ["config", "user.name", "Gate Test"])
            gate.run(prefix + ["config", "user.email", "gate@example.invalid"])
            ignore = temp / "global-ignore"
            ignore.write_text(".private-token\n", encoding="utf-8")
            gate.run(prefix + ["config", "core.excludesFile", str(ignore)])
            (source / "tracked.txt").write_bytes(b"committed\n")
            gate.run(prefix + ["add", "tracked.txt"])
            gate.run(prefix + ["-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
                               "commit", "-m", "fixture"])
            (source / ".private-token").write_text("not-a-real-secret", encoding="utf-8")
            self.assertEqual(gate.run(prefix + ["status", "--porcelain"]).strip(), b"")
            sha = gate.run(prefix + ["rev-parse", "HEAD"]).decode().strip()
            with patch.object(gate, "ROOT", source):
                snapshot = gate.snapshot_checkout(temp, sha, sha, "main", "owner/repo")
            self.assertFalse((snapshot / ".private-token").exists())
            self.assertEqual((snapshot / "tracked.txt").read_bytes(), b"committed\n")
            config = (snapshot / ".git/config").read_text()
            self.assertNotIn("excludesfile", config.lower())
            self.assertNotIn(str(ignore), config)


STARTUP = 'time="2026-11-25T15:22:10-05:00" level=info msg="Start server on http://192.168.1.100:40001"\n'
BIND_FAILURE = ("act failed\n" + STARTUP +
                'time="2026-11-25T15:22:10-05:00" level=fatal msg="listen tcp 192.168.1.100:40001: bind: '
                'Only one usage of each socket address (protocol/network address/port) is normally permitted."\n')


class ArtifactPortTests(unittest.TestCase):
    """Concurrent gates must not share act's default artifact-server port."""

    def replay(self, outcomes, ports=None):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "scripts").mkdir()
            config = {"repository": "owner/repo", "jobs": [{"workflow": "ci.yml", "job": "test"}],
                      "hosted_only": []}
            (root / "scripts/local-gate.json").write_text(json.dumps(config), encoding="utf-8")
            (root / "ci.yml").write_text("jobs: {}", encoding="utf-8")
            pr = {"head": {"sha": "abc", "repo": {"full_name": "owner/repo"}},
                  "base": {"sha": "base", "ref": "main", "repo": {"full_name": "owner/repo"}}}
            replay = []

            def run(args, cwd=None):
                if args[:3] == ["git", "status", "--porcelain"]:
                    return b""
                if args[:3] == ["git", "rev-parse", "HEAD"]:
                    return b"abc"
                if args[:3] == ["git", "rev-parse", "--absolute-git-dir"]:
                    return str(root / ".git").encode()
                if args[:2] == ["gh", "api"]:
                    return json.dumps(pr).encode()
                if args == ["act", "--version"]:
                    return b"act test"
                if args[:2] == ["act", "pull_request"]:
                    replay.append(args)
                    outcome = outcomes.pop(0)
                    if isinstance(outcome, Exception):
                        raise outcome
                    return outcome
                raise AssertionError(args)

            free_port = patch.object(gate, "free_port", side_effect=ports) if ports else patch.object(gate, "free_port", wraps=gate.free_port)
            with patch.object(gate, "ROOT", root), patch.object(gate, "run", side_effect=run),                     patch.object(gate.shutil, "which", return_value="act"),                     patch.object(gate, "snapshot_checkout", return_value=root),                     patch.object(Path, "home", return_value=root),                     patch.object(sys, "argv", ["check.py", "--pr", "1"]), free_port:
                try:
                    gate.main()
                finally:
                    self.replayed = replay
                    report = root / ".git" / "local-gate-evidence.json"
                    self.evidence = json.loads(report.read_text(encoding="utf-8")) if report.exists() else None
                    log = self.evidence and self.evidence["jobs"][0].get("failed_attempt_log")
                    self.failed_attempt_text = Path(log).read_text(encoding="utf-8") if log else None
        return [int(args[args.index("--artifact-server-port") + 1]) for args in self.replayed]

    def test_replay_gets_an_os_assigned_artifact_server_port(self):
        self.assertEqual(len(self.replay([b"Job succeeded"])), 1)
        self.assertGreater(gate.free_port(), 0)

    def test_retries_once_with_a_new_port_on_a_bind_failure(self):
        self.assertEqual(self.replay([RuntimeError(BIND_FAILURE), b"Job succeeded"], ports=[40001, 40002]),
                         [40001, 40002])
        job = self.evidence["jobs"][0]
        self.assertEqual(job["failed_attempt_port"], 40001)
        self.assertTrue(self.failed_attempt_text.startswith("act failed"))
        self.assertEqual(job["failed_attempt_output_sha256"],
                         hashlib.sha256(self.failed_attempt_text.encode("utf-8")).hexdigest())

    def test_job_level_address_in_use_is_not_retried(self):
        # act logs the chosen port on every normal startup; an unrelated EADDRINUSE must not retry.
        failure = RuntimeError(STARTUP + "[CI/test]   | npm test: Error: listen EADDRINUSE: address already in use :::3000\n"
                               "[CI/test]   | listen tcp 127.0.0.1:3000: bind: address already in use\n")
        with self.assertRaises(RuntimeError):
            self.replay([failure, b"Job succeeded"], ports=[40001, 40002])
        self.assertEqual(len(self.replayed), 1)

    def test_job_output_that_quotes_the_listener_error_is_not_retried(self):
        quoted = "".join("[CI/test]   | " + line for line in BIND_FAILURE.splitlines(True)[1:])
        with self.assertRaises(RuntimeError):
            self.replay([RuntimeError(STARTUP + quoted), b"Job succeeded"], ports=[40001, 40002])
        self.assertEqual(len(self.replayed), 1)

    def test_listener_message_quoted_mid_log_is_not_retried(self):
        # e.g. a multi-line step name: later lines carry no job prefix, but the log goes on after them.
        mid_log = STARTUP + BIND_FAILURE.splitlines(True)[-1] + "[CI/test]   \u274c  Failure - Main Tests\n"
        with self.assertRaises(RuntimeError):
            self.replay([RuntimeError(mid_log), b"Job succeeded"], ports=[40001, 40002])
        self.assertEqual(len(self.replayed), 1)

    def test_bind_failure_on_a_different_port_is_not_retried(self):
        other = BIND_FAILURE.replace("40001", "40009")
        with self.assertRaises(RuntimeError):
            self.replay([RuntimeError(other), b"Job succeeded"], ports=[40001, 40002])
        self.assertEqual(len(self.replayed), 1)


if __name__ == "__main__":
    unittest.main()
