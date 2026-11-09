"""Tests for maintain-check. Each builds a throwaway git repository with a base commit.

    python -I -m unittest discover -s workflow-pack/skills/maintain-check/tests
"""
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SKILL = HERE.parent
FIX = SKILL / "fixtures"
sys.path.insert(0, str(SKILL / "scripts"))
import maintain_check  # noqa: E402

FLAGS = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0


def git(repo: Path, *args: str) -> None:
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false", *args],
                   cwd=repo, check=True, capture_output=True, stdin=subprocess.DEVNULL,
                   creationflags=FLAGS)


class Repo(unittest.TestCase):
    """A repository whose base commit holds `base`, with `head` then written on top."""

    def repo(self, base: dict[str, str], head: dict[str, str]) -> Path:
        d = Path(tempfile.mkdtemp(prefix="maintain-check-"))
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        git(d, "init", "-q")
        for rel, text in {"README.md": "base\n", **base}.items():
            (d / rel).parent.mkdir(parents=True, exist_ok=True)
            (d / rel).write_text(text, encoding="utf-8", newline="\n")
        git(d, "add", "-A")
        git(d, "commit", "-q", "-m", "base")
        for rel, text in head.items():
            (d / rel).parent.mkdir(parents=True, exist_ok=True)
            (d / rel).write_text(text, encoding="utf-8", newline="\n")
        return d

    def run_check(self, repo: Path, *extra: str) -> tuple[int, dict]:
        out = repo / ".verdict.json"
        code = maintain_check.main(["--root", str(repo), "--base", "HEAD", "--json", str(out),
                                    "--quiet", *extra])
        return code, json.loads(out.read_text(encoding="utf-8"))

    def rules(self, verdict: dict, bucket: str) -> list[tuple[str, str]]:
        return sorted((f["rule"], f["relpath"]) for f in verdict[bucket])

    def symbols(self, verdict: dict, bucket: str) -> list[tuple[str, str]]:
        return sorted((f["rule"], f["symbol"]) for f in verdict[bucket])


def lines(count: int, prefix: str = "x") -> str:
    return "".join(f"{prefix}_{i} = {i}\n" for i in range(count))


def py_function(name: str, body_lines: int, branches: int = 0) -> str:
    body = "".join(f"    if v == {i}:\n        v += 1\n" for i in range(branches))
    body += "".join(f"    v += {i}\n" for i in range(max(0, body_lines - 2 * branches)))
    return f"def {name}(v):\n{body}    return v\n\n"


class FileGrowth(Repo):
    def test_small_change_passes(self):  # negative control
        repo = self.repo({"a.py": lines(10)}, {"a.py": lines(12)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["verdict"], v["gating"], v["score"]["value"]), (0, "pass", [], 100))

    def test_new_file_over_budget_gates(self):
        repo = self.repo({}, {"big.py": lines(900)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.rules(v, "gating")), (2, [("file-growth", "big.py")]))

    def test_file_that_crosses_the_budget_gates_however_small_the_step(self):
        repo = self.repo({"big.py": lines(798)}, {"big.py": lines(802)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.rules(v, "gating")), (2, [("file-growth", "big.py")]))

    def test_oversized_file_that_grows_past_the_allowance_gates(self):
        repo = self.repo({"big.py": lines(900)}, {"big.py": lines(960)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.rules(v, "gating")), (2, [("file-growth", "big.py")]))
        self.assertEqual((v["gating"][0]["base"], v["gating"][0]["measured"]), (901, 961))

    def test_oversized_file_that_grows_within_the_allowance_is_advisory(self):
        repo = self.repo({"big.py": lines(900)}, {"big.py": lines(905)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["gating"], self.rules(v, "advisory")), (0, [], [("file-growth", "big.py")]))

    def test_oversized_file_that_shrinks_is_preexisting(self):
        repo = self.repo({"big.py": lines(900)}, {"big.py": lines(880)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["gating"], self.rules(v, "preexisting")),
                         (0, [], [("file-growth", "big.py")]))

    def test_untouched_oversized_file_is_not_scanned(self):
        repo = self.repo({"big.py": lines(900), "a.py": lines(5)}, {"a.py": lines(6)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["files"], v["preexisting"]), (0, ["a.py"], []))


class Functions(Repo):
    def test_new_long_python_function_gates(self):
        repo = self.repo({}, {"m.py": py_function("small", 5) + py_function("huge", 140)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.symbols(v, "gating")), (2, [("function-length", "huge")]))

    def test_long_function_left_alone_is_preexisting(self):
        base = py_function("huge", 140) + py_function("small", 5)
        repo = self.repo({"m.py": base}, {"m.py": base + py_function("other", 6)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["gating"], self.symbols(v, "preexisting")),
                         (0, [], [("function-length", "huge")]))

    def test_long_function_that_grows_past_the_allowance_gates(self):
        repo = self.repo({"m.py": py_function("huge", 140)}, {"m.py": py_function("huge", 150)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.symbols(v, "gating")), (2, [("function-length", "huge")]))

    def test_long_function_that_grows_within_the_allowance_is_advisory(self):
        repo = self.repo({"m.py": py_function("huge", 140)}, {"m.py": py_function("huge", 143)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["gating"], self.symbols(v, "advisory")),
                         (0, [], [("function-length", "huge")]))

    def test_branch_heavy_python_function_gates(self):
        repo = self.repo({}, {"m.py": py_function("tangled", 60, branches=25)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.symbols(v, "gating")), (2, [("function-branches", "tangled")]))

    def test_rust_functions_are_measured_from_fixture(self):
        table = maintain_check.functions((FIX / "shapes.rs").read_text(encoding="utf-8"), "rust")
        self.assertEqual(sorted(table), ["declared_only_in_impl", "long_one", "short_one"])
        self.assertEqual(table["short_one"]["lines"], 3)
        self.assertGreater(table["long_one"]["lines"], 100)
        # Braces inside strings, comments, char literals and lifetimes must not end a body early.
        self.assertEqual(table["declared_only_in_impl"]["lines"], 9)
        self.assertEqual(table["declared_only_in_impl"]["branches"], 2)

    def test_new_long_rust_function_gates(self):
        repo = self.repo({}, {"src/lib.rs": (FIX / "shapes.rs").read_text(encoding="utf-8")})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.symbols(v, "gating")), (2, [("function-length", "long_one")]))

    def test_inline_rust_test_module_is_left_out(self):
        body = "".join(f"        let _v{i} = {i};\n" for i in range(900))
        tests = "#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn long_test() {\n" + body + "    }\n}\n"
        source = "pub fn real(a: u32) -> u32 {\n    a + 1\n}\n\n" + tests
        self.assertEqual(sorted(maintain_check.functions(source, "rust")), ["real"])
        self.assertLess(maintain_check.production_lines(source, "rust"), 10)
        repo = self.repo({}, {"src/lib.rs": source, "src/lane_e_tests.rs": lines(900)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["files"], v["gating"]), (0, ["src/lib.rs"], []))

    def test_typescript_function_forms_are_measured_from_fixture(self):
        table = maintain_check.functions((FIX / "shapes.ts").read_text(encoding="utf-8"), "ts")
        self.assertEqual(sorted(table), ["arrow", "declared", "method"])
        self.assertEqual((table["declared"]["lines"], table["arrow"]["lines"], table["method"]["lines"]),
                         (5, 3, 6))
        # Branch points are keywords plus && and ||; a ternary is not counted.
        self.assertEqual(table["method"]["branches"], 2)

    def test_unparseable_python_is_skipped_not_passed_silently(self):
        repo = self.repo({}, {"broken.py": "def oops(:\n    pass\n"})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["skipped"]), (0, [{"relpath": "broken.py", "reason": "could not parse functions"}]))


class Suppressions(Repo):
    def test_new_suppression_gates_and_existing_one_does_not(self):
        base = "import os  # noqa: F401\nvalue = 1\n"
        head = base + "other: int = 'x'  # type: ignore[assignment]\n"
        repo = self.repo({"m.py": base}, {"m.py": head})
        code, v = self.run_check(repo)
        self.assertEqual(code, 2)
        self.assertEqual([(f["rule"], f["kind"], f["line"]) for f in v["gating"]],
                         [("new-suppression", "py-type-ignore", 3)])

    def test_rust_and_typescript_suppressions(self):
        repo = self.repo({}, {"src/a.rs": "#[allow(dead_code)]\nfn a() {}\n",
                              "src/b.ts": "// @ts-ignore\nexport const b: number = 'x';\n"})
        code, v = self.run_check(repo)
        self.assertEqual((code, sorted(f["kind"] for f in v["gating"])), (2, ["rust-allow", "ts-ignore"]))

    def test_moving_a_suppression_is_not_new(self):
        base = "a = 1\nimport os  # noqa: F401\n"
        repo = self.repo({"m.py": base}, {"m.py": "import os  # noqa: F401\na = 1\nb = 2\n"})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["gating"]), (0, []))


class Duplicates(Repo):
    def test_block_added_in_two_files_is_advisory(self):
        block = "".join(f"result_{i} = compute(step_{i}, payload)\n" for i in range(12))
        repo = self.repo({"a.py": "a = 1\n", "b.py": "b = 1\n"},
                         {"a.py": "a = 1\n" + block, "b.py": "b = 1\n" + block})
        code, v = self.run_check(repo)
        self.assertEqual(code, 0)
        self.assertEqual({f["rule"] for f in v["advisory"]}, {"duplicate-block"})
        self.assertEqual(v["score"]["value"], 100)  # advisories never lower the score


class Score(Repo):
    def test_score_is_the_share_of_checked_files_left_no_worse(self):
        repo = self.repo({}, {"big.py": lines(900), "a.py": lines(5), "b.py": lines(5), "c.py": lines(5)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["score"]["value"], v["score"]["files_checked"], v["score"]["files_worse"]),
                         (2, 75, 4, 1))


class Scope(Repo):
    def test_tests_vendored_and_non_code_files_are_out_of_scope(self):
        repo = self.repo({}, {"tests/test_big.py": lines(900), "node_modules/x/big.js": lines(900),
                              "notes.md": lines(900), "src/widget.test.ts": lines(900)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["files"], v["gating"]), (0, [], []))

    def test_explicit_targets_limit_the_scan(self):
        repo = self.repo({}, {"big.py": lines(900), "other.py": lines(900)})
        code, v = self.run_check(repo, str(repo / "big.py"))
        self.assertEqual((code, v["files"]), (2, ["big.py"]))


class Config(Repo):
    def test_reasoned_ignore_clears_a_gate(self):
        config = {"ignores": [{"rule": "file-growth", "path": "gen/*.py",
                               "reason": "Generated table, regenerated by the build"}]}
        repo = self.repo({}, {"gen/table.py": lines(900), ".maintain-check.json": json.dumps(config)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["gating"], self.rules(v, "ignored")),
                         (0, [], [("file-growth", "gen/table.py")]))

    def test_ignore_without_reason_is_an_error(self):
        config = {"ignores": [{"rule": "file-growth", "reason": " "}]}
        repo = self.repo({}, {"big.py": lines(900), ".maintain-check.json": json.dumps(config)})
        code, v = self.run_check(repo)
        self.assertEqual((code, v["verdict"]), (1, "error"))

    def test_budget_override(self):
        config = {"budgets": {"file_lines": 50}}
        repo = self.repo({}, {"a.py": lines(60), ".maintain-check.json": json.dumps(config)})
        code, v = self.run_check(repo)
        self.assertEqual((code, self.rules(v, "gating")), (2, [("file-growth", "a.py")]))


class Availability(Repo):
    def test_outside_a_repository_is_unavailable_never_pass(self):
        d = Path(tempfile.mkdtemp(prefix="maintain-check-norepo-"))
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        out = d / "v.json"
        code = maintain_check.main(["--root", str(d), "--json", str(out), "--quiet"])
        self.assertEqual((code, json.loads(out.read_text(encoding="utf-8"))["verdict"]), (1, "unavailable"))

    def test_unknown_base_is_unavailable(self):
        repo = self.repo({}, {"a.py": lines(5)})
        out = repo / ".verdict.json"
        code = maintain_check.main(["--root", str(repo), "--base", "no-such-ref", "--json", str(out), "--quiet"])
        self.assertEqual((code, json.loads(out.read_text(encoding="utf-8"))["verdict"]), (1, "unavailable"))


class Logging(Repo):
    def test_run_row_label_and_rates(self):
        repo = self.repo({}, {"big.py": lines(900)})
        log = repo / "state" / "maintain-check.jsonl"
        code, v = self.run_check(repo, "--log", str(log), "--session", "s1", "--task", "t1", "--mode", "block")
        self.assertEqual((code, v["mode"]), (2, "block"))
        row = json.loads(log.read_text(encoding="utf-8").splitlines()[0])
        self.assertEqual((row["verdict"], row["counts"]["gating"], row["task_id"]), ("block", 1, "t1"))
        key = v["gating"][0]["finding_key"]
        self.assertEqual(maintain_check.main(["label", "--log", str(log), "--run", row["run_id"],
                                              "--finding", key, "--label", "fp", "--by", "reviewer"]), 0)
        report = maintain_check.rates(maintain_check.read_rows(str(log)))
        self.assertEqual(report["rules"]["file-growth"], {"hits": 1, "labeled": 1, "fp": 1, "tp": 0, "fp_rate": 1.0})
        self.assertEqual(report["verdicts"], {"block": 1})

    def test_partial_log_flags_are_an_error(self):
        repo = self.repo({}, {"a.py": lines(5)})
        code, v = self.run_check(repo, "--log", str(repo / "x.jsonl"))
        self.assertEqual((code, v["verdict"]), (1, "error"))


class OwnBudget(unittest.TestCase):
    def test_the_checker_fits_its_own_default_budgets(self):
        text = (SKILL / "scripts" / "maintain_check.py").read_text(encoding="utf-8")
        self.assertLessEqual(text.count("\n") + 1, maintain_check.DEFAULT_BUDGETS["file_lines"])
        for name, info in maintain_check.functions(text, "python").items():
            self.assertLessEqual(info["lines"], maintain_check.DEFAULT_BUDGETS["function_lines"], name)
            self.assertLessEqual(info["branches"], maintain_check.DEFAULT_BUDGETS["function_branches"], name)


if __name__ == "__main__":
    unittest.main()
