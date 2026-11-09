"""maintain-check: a delta gate for code maintainability.

Compares the working tree with a base revision and reports only what the change
introduced: files and functions that grew past a budget, new branch-heavy
functions, new lint or type suppressions, and newly duplicated blocks. Debt that
already existed at the base is reported as preexisting and never gates.

Standard library only. Read-only on the repository.

Exit codes: 0 pass, 2 block (gating findings), 1 error or unavailable.
"""
from __future__ import annotations

import argparse
import ast
import fnmatch
import glob
import hashlib
import json
import os
import re
import subprocess
import sys
import uuid
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

VERDICT_VERSION = 1
CONFIG_FILE = ".maintain-check.json"
BUCKETS = ("gating", "advisory", "ignored", "preexisting")
DEFAULT_BUDGETS = {"file_lines": 800, "function_lines": 100, "function_branches": 20,
                   "duplicate_lines": 8,
                   # How far something already over its budget may grow before it gates.
                   "file_growth_allowance": 20, "function_growth_allowance": 5,
                   "branch_growth_allowance": 2}
GATING_RULES = ("file-growth", "function-length", "function-branches", "new-suppression")

BRACE_SUFFIXES = {".rs": "rust", ".ts": "ts", ".tsx": "ts", ".js": "ts", ".jsx": "ts",
                  ".mjs": "ts", ".cjs": "ts", ".svelte": "ts", ".vue": "ts"}
OTHER_SUFFIXES = {".py": "python", ".go": "other", ".java": "other", ".cs": "other",
                  ".kt": "other", ".swift": "other", ".rb": "other", ".php": "other"}
SKIP_DIRS = {"node_modules", "target", "dist", "build", "vendor", ".git", ".hive-manager",
             ".claude", ".agents", ".worktrees", "worktrees", "__pycache__", ".svelte-kit",
             ".next", "coverage"}
TEST_RE = re.compile(r"(^|/)(tests?|__tests__|fixtures|testdata)/|(^|/)test_[^/]*$|"
                     r"(^|/)tests(_[^/]*)?\.rs$|[._](test|spec)\.[^/]+$|_tests?\.[^/]+$")
SUPPRESSIONS = [
    ("rust-allow", {"rust"}, re.compile(r"#!?\[\s*allow\s*\(")),
    ("ts-ignore", {"ts"}, re.compile(r"@ts-(ignore|expect-error|nocheck)\b")),
    ("eslint-disable", {"ts"}, re.compile(r"\beslint-disable\b")),
    ("py-noqa", {"python"}, re.compile(r"#\s*noqa\b")),
    ("py-type-ignore", {"python"}, re.compile(r"#\s*type:\s*ignore\b")),
    ("py-lint-disable", {"python"}, re.compile(r"#\s*pylint:\s*disable\b")),
]
BRANCH_RE = re.compile(r"\b(if|for|while|match|case|catch|loop)\b|&&|\|\|")
RUST_FN_RE = re.compile(r"\bfn\s+([A-Za-z_]\w*)")
RUST_TEST_MOD_RE = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?mod\s+\w+\s*\{")
TS_FN_RES = [
    re.compile(r"\bfunction\s*\*?\s*([A-Za-z_$][\w$]*)\s*(?:<[^>(]*>)?\s*\("),
    re.compile(r"\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=;]+)?=\s*(?:async\s+)?"
               r"(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*(?::[^=;{]+)?=>\s*(?=\{)"),
    re.compile(r"^[ \t]*(?:(?:public|private|protected|static|async|override|get|set)\s+)*"
               r"([A-Za-z_$][\w$]*)\s*(?:<[^>(]*>)?\([^)]*\)\s*(?::[^{;=]+)?(?=\{)", re.M),
]
TS_NOT_FUNCTIONS = {"if", "for", "while", "switch", "catch", "function", "return", "else",
                    "do", "try", "with", "constructor"}


class Unavailable(Exception):
    """The comparison could not be made; never a pass."""


def git(root: Path, *args: str) -> subprocess.CompletedProcess:
    flags = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
    try:
        return subprocess.run(["git", *args], cwd=root, capture_output=True, timeout=120,
                              stdin=subprocess.DEVNULL, creationflags=flags)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise Unavailable(f"git {args[0]} failed: {exc}") from exc


def repo_root(start: Path) -> Path:
    out = git(start, "rev-parse", "--show-toplevel")
    if out.returncode != 0:
        raise Unavailable("not inside a git repository")
    return Path(out.stdout.decode("utf-8", "replace").strip())


def resolve_base(root: Path, explicit: str | None) -> str:
    candidates = [explicit] if explicit else []
    if not explicit:
        head = git(root, "symbolic-ref", "--quiet", "refs/remotes/origin/HEAD")
        if head.returncode == 0:
            candidates.append(head.stdout.decode().strip().replace("refs/remotes/", ""))
        candidates += ["origin/staging", "origin/main", "origin/master"]
    for ref in candidates:
        probe = git(root, "merge-base", "HEAD", ref) if not explicit else \
            git(root, "rev-parse", "--verify", "--quiet", ref + "^{commit}")
        sha = probe.stdout.decode().strip()
        if probe.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}", sha):
            return sha
    raise Unavailable("no base revision: pass --base or fetch the default branch")


def changed_files(root: Path, base: str) -> list[str]:
    diff = git(root, "diff", "--name-only", "-z", "--diff-filter=ACMR", base)
    new = git(root, "ls-files", "--others", "--exclude-standard", "-z")
    if diff.returncode != 0 or new.returncode != 0:
        raise Unavailable("cannot list changed files")
    names = (diff.stdout + new.stdout).decode("utf-8", "replace").split("\0")
    return sorted({n for n in names if n})


def base_text(root: Path, base: str, rel: str) -> str | None:
    out = git(root, "show", f"{base}:{rel}")
    return out.stdout.decode("utf-8", "replace") if out.returncode == 0 else None


def language(rel: str) -> str | None:
    suffix = Path(rel).suffix.lower()
    return BRACE_SUFFIXES.get(suffix) or OTHER_SUFFIXES.get(suffix)


def in_scope(rel: str) -> bool:
    parts = rel.split("/")
    if any(p in SKIP_DIRS for p in parts[:-1]) or language(rel) is None:
        return False
    return not TEST_RE.search(rel) and not rel.endswith((".min.js", ".d.ts"))


def _line_comment(text: str, i: int, lang: str):
    if text[i:i + 2] != "//":
        return None
    j = text.find("\n", i)
    j = len(text) if j < 0 else j
    return i, j, j


def _block_comment(text: str, i: int, lang: str):
    if text[i:i + 2] != "/*":
        return None
    depth, j = 1, i + 2
    while j < len(text) and depth:
        pair = text[j:j + 2]
        if pair == "/*" and lang == "rust":  # Rust block comments nest
            depth, j = depth + 1, j + 2
        elif pair == "*/":
            depth, j = depth - 1, j + 2
        else:
            j += 1
    return i, j, j


def _raw_string(text: str, i: int, lang: str):
    if lang != "rust" or text[i] != "r" or (i and (text[i - 1].isalnum() or text[i - 1] == "_")):
        return None
    opening = re.match(r'r(#*)"', text[i:i + 12])
    if not opening:
        return None
    closing = '"' + opening.group(1)
    end = text.find(closing, i + opening.end())
    end = len(text) if end < 0 else end + len(closing)
    return i + 1, end, end


def _quoted(text: str, i: int, lang: str):
    if text[i] != '"' and not (text[i] in "'`" and lang == "ts"):
        return None
    j = i + 1
    while j < len(text) and text[j] != text[i]:
        j += 2 if text[j] == "\\" else 1
    return i + 1, j, j + 1


def _rust_char(text: str, i: int, lang: str):
    """A char literal. A lone quote is a lifetime and is left alone."""
    if lang != "rust" or text[i] != "'":
        return None
    char = re.match(r"'(\\.[^']*|[^\\'])'", text[i:i + 12])
    return (i + 1, i + char.end() - 1, i + char.end()) if char else None


SCANNERS = (_line_comment, _block_comment, _raw_string, _quoted, _rust_char)


def strip_code(text: str, lang: str) -> str:
    """Blank out comments and string contents, keeping every newline and offset."""
    out, i, n = list(text), 0, len(text)
    while i < n:
        span = None
        if text[i] in "/\"'`r":
            span = next((s for s in (scan(text, i, lang) for scan in SCANNERS) if s), None)
        if span is None:
            i += 1
            continue
        start, stop, i = span
        for k in range(start, min(stop, n)):
            if out[k] != "\n":
                out[k] = " "
    return "".join(out)


def body_end(code: str, open_at: int) -> int:
    depth = 0
    for k in range(open_at, len(code)):
        if code[k] == "{":
            depth += 1
        elif code[k] == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(code) - 1


def body_start(code: str, at: int) -> int | None:
    """First `{` after a signature, or None for a bodiless declaration."""
    depth = 0
    for k in range(at, len(code)):
        ch = code[k]
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
        elif depth == 0 and ch == ";":
            return None
        elif depth == 0 and ch == "{":
            return k
    return None


def test_spans(code: str, lang: str) -> list[tuple[int, int]]:
    """Character ranges of inline `#[cfg(test)] mod` blocks in stripped Rust code."""
    if lang != "rust":
        return []
    return [(m.start(), body_end(code, m.end() - 1)) for m in RUST_TEST_MOD_RE.finditer(code)]


def production_lines(text: str | None, lang: str | None) -> int:
    """Line count with inline test modules left out."""
    if text is None:
        return 0
    total = text.count("\n") + 1
    if lang != "rust":
        return total
    code = strip_code(text, lang)
    return total - sum(code.count("\n", a, b) + 1 for a, b in test_spans(code, lang))


def brace_functions(text: str, lang: str) -> dict[str, dict]:
    code = strip_code(text, lang)
    spans = []
    tests = test_spans(code, lang)
    if lang == "rust":
        for m in RUST_FN_RE.finditer(code):
            start = body_start(code, m.end())
            if start is not None and not any(a <= m.start() <= b for a, b in tests):
                spans.append((m.group(1), m.start(), start))
    else:
        for pattern in TS_FN_RES:
            for m in pattern.finditer(code):
                if m.group(1) in TS_NOT_FUNCTIONS:
                    continue
                # A declaration's match ends at its parameter list; the others end at the body.
                start = body_start(code, m.end() - 1) if pattern is TS_FN_RES[0] else m.end()
                if start is not None and code[start:start + 1] == "{":
                    spans.append((m.group(1), m.start(), start))
    found, seen, taken = {}, Counter(), set()
    for name, head, start in sorted(spans, key=lambda s: s[1]):
        if start in taken:
            continue
        taken.add(start)
        end = body_end(code, start)
        seen[name] += 1
        key = name if seen[name] == 1 else f"{name}#{seen[name]}"
        line = code.count("\n", 0, head) + 1
        found[key] = {"line": line, "lines": code.count("\n", head, end) + 1,
                      "branches": len(BRANCH_RE.findall(code[start:end]))}
    return found


def python_functions(text: str) -> dict[str, dict]:
    tree = ast.parse(text)
    found, seen = {}, Counter()

    def branches(node: ast.AST) -> int:
        total = 0
        for child in ast.walk(node):
            if isinstance(child, (ast.If, ast.For, ast.AsyncFor, ast.While, ast.IfExp,
                                  ast.ExceptHandler, ast.match_case)):
                total += 1
            elif isinstance(child, ast.BoolOp):
                total += len(child.values) - 1
            elif isinstance(child, ast.comprehension):
                total += 1 + len(child.ifs)
        return total

    def visit(node: ast.AST, prefix: str) -> None:
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                name = prefix + child.name
                seen[name] += 1
                key = name if seen[name] == 1 else f"{name}#{seen[name]}"
                found[key] = {"line": child.lineno, "branches": branches(child),
                              "lines": (child.end_lineno or child.lineno) - child.lineno + 1}
                visit(child, name + ".")
            elif isinstance(child, ast.ClassDef):
                visit(child, prefix + child.name + ".")
            else:
                visit(child, prefix)

    visit(tree, "")
    return found


def functions(text: str | None, lang: str) -> dict[str, dict] | None:
    """Function table, or None when this file cannot be parsed for functions."""
    if text is None:
        return {}
    try:
        if lang == "python":
            return python_functions(text)
        if lang in ("rust", "ts"):
            return brace_functions(text, lang)
    except (SyntaxError, ValueError, RecursionError):
        return None
    return {}


def finding(rule: str, rel: str, symbol: str, line: int, detail: str, **extra) -> dict:
    identity = json.dumps([rule, rel, symbol], ensure_ascii=False, separators=(",", ":"))
    return {"rule": rule, "relpath": rel, "symbol": symbol, "line": line, "detail": detail,
            "finding_key": hashlib.sha256(identity.encode("utf-8")).hexdigest(), **extra}


def over_budget(rule: str, rel: str, symbol: str, line: int, label: str, unit: str,
                now: int, before: int | None, limit: int, allowance: int) -> tuple[str, dict]:
    """Classify one measurement that is over its budget against the base measurement."""
    extra = {"measured": now, "base": before or 0, "budget": limit}
    if before is None or before <= limit:
        what = "new" if before is None else f"was {before}"
        return "gating", finding(rule, rel, symbol, line,
                                 f"{label}{now} {unit} ({what}), over the budget of {limit}", **extra)
    if now <= before:
        return "preexisting", finding(rule, rel, symbol, line,
                                      f"{label}{now} {unit}, over the budget of {limit}, not grown by this change",
                                      **extra)
    grew = f"{label}{now} {unit} (grew from {before}), already over the budget of {limit}"
    if now - before <= allowance:
        return "advisory", finding(rule, rel, symbol, line, grew + f"; within the allowance of {allowance}", **extra)
    return "gating", finding(rule, rel, symbol, line, grew + f"; more than the allowance of {allowance}", **extra)


def check_file_growth(rel: str, head: str, base: str | None, lang: str | None,
                      budgets: dict) -> list[tuple[str, dict]]:
    now = production_lines(head, lang)
    if now <= budgets["file_lines"]:
        return []
    before = production_lines(base, lang) if base is not None else None
    return [over_budget("file-growth", rel, "", 1, "", "lines", now, before,
                        budgets["file_lines"], budgets["file_growth_allowance"])]


def check_functions(rel: str, head: dict, base: dict, budgets: dict) -> list[tuple[str, dict]]:
    results = []
    for rule, field, unit, limit, allowance in (
            ("function-length", "lines", "lines", budgets["function_lines"],
             budgets["function_growth_allowance"]),
            ("function-branches", "branches", "branch points", budgets["function_branches"],
             budgets["branch_growth_allowance"])):
        for name, info in head.items():
            if info[field] > limit:
                results.append(over_budget(rule, rel, name, info["line"], f"{name}: ", unit, info[field],
                                           base.get(name, {}).get(field), limit, allowance))
    return results


def check_suppressions(rel: str, head: str, base: str | None, lang: str) -> list[tuple[str, dict]]:
    results = []
    base_lines = (base or "").splitlines()
    for kind, langs, pattern in SUPPRESSIONS:
        if lang not in langs:
            continue
        known = Counter(line.strip() for line in base_lines if pattern.search(line))
        for number, line in enumerate(head.splitlines(), 1):
            if not pattern.search(line):
                continue
            text = line.strip()
            if known[text] > 0:
                known[text] -= 1
                continue
            results.append(("gating", finding("new-suppression", rel, f"{kind}:{text[:120]}", number,
                                              f"new {kind} suppression: {text[:120]}", kind=kind)))
    return results


def significant(text: str) -> list[tuple[int, str]]:
    rows = []
    for number, line in enumerate(text.splitlines(), 1):
        norm = " ".join(line.split())
        if len(norm) > 3 and not norm.startswith(("//", "#", "*", "/*", "import ", "use ", "from ")):
            rows.append((number, norm))
    return rows


def windows(text: str, size: int) -> dict[str, int]:
    rows = significant(text)
    table: dict[str, int] = {}
    for i in range(len(rows) - size + 1):
        digest = hashlib.sha256("\n".join(r[1] for r in rows[i:i + size]).encode("utf-8")).hexdigest()
        table.setdefault(digest, rows[i][0])
    return table


def check_duplicates(files: dict[str, tuple[str, str | None]], size: int) -> list[tuple[str, dict]]:
    """Blocks this change added that already exist elsewhere in the changed files."""
    where: dict[str, list[tuple[str, int]]] = {}
    fresh: dict[str, set[str]] = {}
    for rel, (head, base) in files.items():
        head_windows = windows(head, size)
        old = set(windows(base, size)) if base is not None else set()
        fresh[rel] = set(head_windows) - old
        for digest, line in head_windows.items():
            where.setdefault(digest, []).append((rel, line))
    results, reported = [], set()
    for rel in sorted(fresh):
        for digest in fresh[rel]:
            places = where[digest]
            others = [p for p in places if p[0] != rel]
            line = next(p[1] for p in places if p[0] == rel)
            if not others or (rel, line // size) in reported:
                continue
            reported.add((rel, line // size))
            other = others[0]
            results.append(("advisory", finding(
                "duplicate-block", rel, f"{other[0]}:{other[1]}", line,
                f"{size}+ lines added here also appear at {other[0]}:{other[1]}")))
    return results


def load_config(root: Path) -> tuple[dict, list[dict], list[str]]:
    budgets, ignores, problems = dict(DEFAULT_BUDGETS), [], []
    path = root / CONFIG_FILE
    if not path.is_file():
        return budgets, ignores, problems
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return budgets, ignores, [f"{CONFIG_FILE}: unreadable ({exc})"]
    for key, value in (data.get("budgets") or {}).items():
        if key not in DEFAULT_BUDGETS or not isinstance(value, int) or value < 1:
            problems.append(f"{CONFIG_FILE} budgets.{key}: must be a known budget with a positive integer")
        else:
            budgets[key] = value
    for i, entry in enumerate(data.get("ignores") or []):
        if not isinstance(entry, dict) or not entry.get("rule") or not str(entry.get("reason", "")).strip():
            problems.append(f"{CONFIG_FILE} ignores[{i}]: needs both 'rule' and a non-empty 'reason'")
        else:
            ignores.append(entry)
    return budgets, ignores, problems


def ignore_for(item: dict, ignores: list[dict]) -> dict | None:
    for entry in ignores:
        if entry["rule"] != item["rule"]:
            continue
        if entry.get("path") and not fnmatch.fnmatch(item["relpath"], entry["path"]):
            continue
        if entry.get("symbol") and entry["symbol"] not in item["symbol"]:
            continue
        return entry
    return None


def scan(root: Path, base: str, targets: list[str]) -> dict:
    budgets, ignores, problems = load_config(root)
    verdict = {"verdict_version": VERDICT_VERSION, "tool": "maintain-check", "base_sha": base,
               "budgets": budgets, "files": [], "skipped": [], **{b: [] for b in BUCKETS},
               "errors": list(problems)}
    texts: dict[str, tuple[str, str | None]] = {}
    for rel in targets:
        if not in_scope(rel):
            continue
        path = root / rel
        if not path.is_file():
            continue
        head = path.read_text(encoding="utf-8", errors="replace")
        base_version = base_text(root, base, rel)
        texts[rel] = (head, base_version)
        lang = language(rel)
        results = check_file_growth(rel, head, base_version, lang, budgets)
        results += check_suppressions(rel, head, base_version, lang)
        head_functions = functions(head, lang)
        base_functions = functions(base_version, lang)
        if head_functions is None:
            verdict["skipped"].append({"relpath": rel, "reason": "could not parse functions"})
        else:
            results += check_functions(rel, head_functions, base_functions or {}, budgets)
        verdict["files"].append(rel)
        for bucket, item in results:
            entry = ignore_for(item, ignores) if bucket == "gating" else None
            if entry:
                verdict["ignored"].append({**item, "reason": entry["reason"]})
            else:
                verdict[bucket].append(item)
    for bucket, item in check_duplicates(texts, budgets["duplicate_lines"]):
        verdict[bucket].append(item)
    return verdict


def score(verdict: dict) -> dict:
    """Share of the checked files that this change left no worse, as a whole percent."""
    checked = len(verdict["files"])
    worse = len({f["relpath"] for f in verdict["gating"]})
    value = 100 if not checked else round(100 * (checked - worse) / checked)
    return {"value": value, "scale": "0-100", "files_checked": checked, "files_worse": worse,
            "basis": "percent of checked files with no new gating finding"}


def timestamp() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def append_row(path: str, row: dict) -> None:
    destination = Path(path)
    if not destination.is_absolute():
        raise ValueError("--log must be an absolute path")
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = (json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
    fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_APPEND | getattr(os, "O_BINARY", 0), 0o600)
    try:
        if os.write(fd, data) != len(data):
            raise OSError("short log append; row may be incomplete")
    finally:
        os.close(fd)


def log_run(a: argparse.Namespace, verdict: dict) -> None:
    append_row(a.log, {
        "row_version": 1, "type": "run", "tool": "maintain-check", "run_id": str(uuid.uuid4()),
        "session_id": a.session, "task_id": a.task, "mode": a.mode, "verdict": verdict["verdict"],
        "score": verdict.get("score", {}).get("value"), "base_sha": verdict.get("base_sha"),
        "counts": {b: len(verdict[b]) for b in BUCKETS},
        "rules": {b: dict(Counter(f["rule"] for f in verdict[b])) for b in BUCKETS},
        "findings": {f["finding_key"]: f["rule"] for b in ("gating", "advisory") for f in verdict[b]},
        "verdict_path": str(Path(a.out).resolve()) if a.out else None, "timestamp": timestamp()})


def read_rows(pattern: str) -> list[dict]:
    paths = sorted({str(Path(p).resolve()) for p in ([pattern] if Path(pattern).is_file() else glob.glob(pattern))})
    if not paths:
        raise ValueError("--log did not match any files")
    rows = []
    for path in paths:
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            if line.strip():
                rows.append(json.loads(line))
    return rows


def rates(rows: list[dict]) -> dict:
    """Hits and labelled false-positive rate per rule, plus how often the gate could not run."""
    runs = {r["run_id"]: r for r in rows if r.get("type") == "run"}
    result: dict[str, dict] = {}
    for run in runs.values():
        for bucket in ("gating", "advisory"):
            for rule, count in run.get("rules", {}).get(bucket, {}).items():
                result.setdefault(rule, {"hits": 0, "labeled": 0, "fp": 0, "tp": 0, "fp_rate": None})["hits"] += count
    latest = {}
    for row in rows:
        if row.get("type") == "label" and row.get("run_id") in runs:
            latest[(row["run_id"], row["finding_key"])] = row
    for (run_id, key), row in latest.items():
        rule = runs[run_id].get("findings", {}).get(key)
        if rule in result and row.get("label") in ("fp", "tp"):
            result[rule]["labeled"] += 1
            result[rule][row["label"]] += 1
    for item in result.values():
        item["fp_rate"] = item["fp"] / item["labeled"] if item["labeled"] else None
    verdicts = Counter(r.get("verdict") for r in runs.values())
    return {"runs": len(runs), "verdicts": dict(verdicts), "rules": dict(sorted(result.items()))}


def reporting_main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="maintain-check labels and rates")
    sub = ap.add_subparsers(dest="command", required=True)
    label = sub.add_parser("label")
    for flag in ("--log", "--run", "--finding", "--by"):
        label.add_argument(flag, required=True)
    label.add_argument("--label", choices=("fp", "tp"), required=True)
    sub.add_parser("rates").add_argument("--log", required=True)
    a = ap.parse_args(argv)
    try:
        rows = read_rows(a.log)
        if a.command == "rates":
            print(json.dumps(rates(rows), ensure_ascii=False))
            return 0
        run = next((r for r in reversed(rows) if r.get("type") == "run" and r.get("run_id") == a.run), None)
        if not run or a.finding not in run.get("findings", {}) or not a.by.strip():
            raise ValueError("label requires a known run, finding, and non-empty --by")
        row = {"row_version": 1, "type": "label", "run_id": a.run, "finding_key": a.finding,
               "label": a.label, "by": a.by, "timestamp": timestamp()}
        append_row(a.log, row)
        print(json.dumps(row, ensure_ascii=False))
        return 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"maintain-check: reporting error: {exc}", file=sys.stderr)
        return 1


def build_verdict(a: argparse.Namespace) -> dict:
    empty = {"verdict_version": VERDICT_VERSION, "tool": "maintain-check", "base_sha": None,
             "files": [], "skipped": [], **{b: [] for b in BUCKETS}, "errors": []}
    try:
        root = repo_root(Path(a.root) if a.root else Path.cwd())
        base = resolve_base(root, a.base)
        if a.targets:
            targets = sorted({Path(t).resolve().relative_to(root.resolve()).as_posix() for t in a.targets})
        else:
            targets = changed_files(root, base)
        verdict = scan(root, base, targets)
    except Unavailable as exc:
        return {**empty, "errors": [str(exc)], "verdict": "unavailable"}
    except (OSError, ValueError) as exc:
        return {**empty, "errors": [str(exc)], "verdict": "error"}
    verdict["verdict"] = "error" if verdict["errors"] else ("block" if verdict["gating"] else "pass")
    verdict["score"] = score(verdict)
    return verdict


def write_outputs(a: argparse.Namespace, verdict: dict) -> None:
    log_flags = (a.log, a.session, a.task, a.mode)
    if any(log_flags) and not all(log_flags):
        verdict["errors"].append("--log --session --task --mode must be supplied together")
        verdict["verdict"] = "error"
    if a.out:
        try:
            Path(a.out).parent.mkdir(parents=True, exist_ok=True)
            Path(a.out).write_text(json.dumps(verdict, indent=1, ensure_ascii=False), encoding="utf-8")
        except OSError as exc:
            verdict["errors"].append(f"verdict output: {exc}")
            verdict["verdict"] = "error"
    if all(log_flags):
        try:
            log_run(a, verdict)
        except (OSError, ValueError) as exc:
            verdict["errors"].append(f"log: {exc}")
            verdict["verdict"] = "error"


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in ("label", "rates"):
        return reporting_main(argv)
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("targets", nargs="*", help="files to check (default: every file changed since the base)")
    ap.add_argument("--root", help="a path inside the repository (default: the current directory)")
    ap.add_argument("--base", help="base revision (default: merge-base with the default branch)")
    ap.add_argument("--json", "--out", dest="out", help="write the full JSON verdict here")
    ap.add_argument("--log")
    ap.add_argument("--session")
    ap.add_argument("--task")
    ap.add_argument("--mode", choices=("report", "block"))
    ap.add_argument("--quiet", action="store_true")
    a = ap.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    verdict = build_verdict(a)
    verdict["mode"] = a.mode or "report"
    write_outputs(a, verdict)
    if a.quiet:
        value = verdict.get("score", {}).get("value", "n/a")
        print(f"maintain-check: {verdict['verdict']} | score {value}"
              + "".join(f" | {b} {len(verdict[b])}" for b in BUCKETS))
        for item in verdict["gating"]:
            print(f"  {item['relpath']}:{item['line']}: [{item['rule']}] {item['detail']}")
        for error in verdict["errors"]:
            print(f"  error: {error}")
    else:
        print(json.dumps(verdict, indent=1, ensure_ascii=False))
    return {"pass": 0, "block": 2}.get(verdict["verdict"], 1)


if __name__ == "__main__":
    sys.exit(main())
