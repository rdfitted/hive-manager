#!/usr/bin/env python3
"""Turn codegraph reachability findings into a CI exit-code contract."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


ANALYZER = Path(__file__).with_name("codegraph.py")
ANALYZER_TIMEOUT_SECONDS = 120
MODULE_COUNT = re.compile(r"^modules:\s*(\d+)\s*$", re.MULTILINE)
EMPTY_PARSE = re.compile(r"NO .+ MODULES FOUND")


def run_codegraph(command: str, *, root: Path, lang: str) -> subprocess.CompletedProcess[str]:
    arguments = [
        sys.executable,
        str(ANALYZER),
        command,
        "--root",
        str(root),
        "--lang",
        lang,
    ]
    if command == "dead":
        arguments.append("--json")
    return subprocess.run(
        arguments,
        capture_output=True,
        text=True,
        check=False,
        timeout=ANALYZER_TIMEOUT_SECONDS,
    )


def combined_output(process: subprocess.CompletedProcess[str]) -> str:
    return "\n".join(part.strip() for part in (process.stdout, process.stderr) if part.strip())


def parse_findings(process: subprocess.CompletedProcess[str]) -> list[dict[str, Any]]:
    try:
        findings = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(f"dead --json returned invalid JSON: {error.msg}") from error
    if not isinstance(findings, list) or any(not isinstance(row, dict) for row in findings):
        raise ValueError("dead --json returned an unexpected schema")
    required = {"path", "tier", "why"}
    if any(not required.issubset(row) for row in findings):
        raise ValueError("dead --json omitted path, tier, or why")
    return findings


def report_error(message: str, *, advisory: bool) -> int:
    print(f"reachability:error:{message}", file=sys.stderr)
    if advisory:
        print("Reachability advisory reported an error; continuing without failing.")
        return 0
    return 2


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--lang", required=True, choices=("py", "ts", "rs"))
    parser.add_argument(
        "--advisory",
        action="store_true",
        help="report policy findings and analyzer errors without failing",
    )
    args = parser.parse_args()

    try:
        stats = run_codegraph("stats", root=args.root, lang=args.lang)
        dead = run_codegraph("dead", root=args.root, lang=args.lang)
    except subprocess.TimeoutExpired as error:
        command = error.cmd[2]
        return report_error(
            f"{command} timed out after {error.timeout:g} seconds",
            advisory=args.advisory,
        )
    stats_output = combined_output(stats)
    dead_output = combined_output(dead)
    empty_parse = bool(EMPTY_PARSE.search(stats_output) or EMPTY_PARSE.search(dead_output))

    count_match = MODULE_COUNT.search(stats.stdout)
    module_count = int(count_match.group(1)) if count_match else 0
    if empty_parse or module_count == 0:
        detail = dead_output or stats_output or "analyzer reported zero modules"
        print(f"reachability:empty:{detail}", file=sys.stderr)
        if args.advisory:
            print("Reachability advisory found no modules; continuing without failing.")
            return 0
        return 1

    if stats.returncode != 0:
        return report_error(
            f"stats exited {stats.returncode}: {stats_output}", advisory=args.advisory
        )
    if dead.returncode != 0:
        return report_error(
            f"dead --json exited {dead.returncode}: {dead_output}", advisory=args.advisory
        )

    try:
        findings = parse_findings(dead)
    except ValueError as error:
        return report_error(str(error), advisory=args.advisory)

    for finding in findings:
        print(f"{finding['path']}:{finding['tier']}:{finding['why']}")

    certain_count = sum(finding["tier"] == "certain" for finding in findings)
    print(
        f"Reachability analyzed {module_count} {args.lang} modules: "
        f"{len(findings)} finding(s), {certain_count} certain."
    )
    if certain_count and not args.advisory:
        return 1
    if args.advisory:
        print("Reachability advisory complete; findings do not fail this job.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
