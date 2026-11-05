"""Local replay must not inherit the GitHub credential or host daemon socket."""
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
            (source / "tracked.txt").write_text("committed", encoding="utf-8")
            gate.run(prefix + ["add", "tracked.txt"])
            gate.run(prefix + ["-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
                               "commit", "-m", "fixture"])
            (source / ".private-token").write_text("not-a-real-secret", encoding="utf-8")
            self.assertEqual(gate.run(prefix + ["status", "--porcelain"]).strip(), b"")
            sha = gate.run(prefix + ["rev-parse", "HEAD"]).decode().strip()
            with patch.object(gate, "ROOT", source):
                snapshot = gate.snapshot_checkout(temp, sha, sha, "main", "owner/repo")
            self.assertFalse((snapshot / ".private-token").exists())
            self.assertEqual((snapshot / "tracked.txt").read_text(), "committed")
            config = (snapshot / ".git/config").read_text()
            self.assertNotIn("excludesfile", config.lower())
            self.assertNotIn(str(ignore), config)


if __name__ == "__main__":
    unittest.main()
