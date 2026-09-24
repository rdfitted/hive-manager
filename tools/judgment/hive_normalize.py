#!/usr/bin/env python3
"""Copy legacy hive QA rows to canonical shape in a separate JSONL ledger.

The input is never modified. Use --output in a temporary directory when
auditing a live ledger; audit.py then reads the copied ledger explicitly.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
from pathlib import Path

import ledger


SURFACE = "hive.qa.criterion"
RESULTS = {"pass", "fail"}


def _decision_ids(source: Path) -> set[str]:
    ids = set()
    with source.open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            row = json.loads(line)
            if row.get("kind") == "decision" and row.get("surface") == SURFACE:
                answer = row.get("answer")
                if not isinstance(answer, dict) or "result" not in answer:
                    raise ValueError(f"ambiguous QA answer at line {line_number}")
                if set(answer) not in ({"result"}, {"result", "rationale"}):
                    raise ValueError(f"ambiguous QA answer at line {line_number}")
                ids.add(row["decision_id"])
    return ids


def normalize_ledger(source: Path, output: Path) -> tuple[int, int]:
    source = source.resolve(strict=True)
    output = output.resolve()
    if source == output or output == ledger.ledger_path().resolve():
        raise ValueError("output must be a separate temporary ledger")
    if output.exists():
        raise FileExistsError(f"output already exists: {output}")
    qa_ids = _decision_ids(source)
    output.parent.mkdir(parents=True, exist_ok=True)
    changed = total = 0
    temporary_name = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", newline="\n", suffix=".jsonl",
            prefix="hive-normalized-", dir=output.parent, delete=False,
        ) as target:
            temporary_name = target.name
            with source.open(encoding="utf-8") as stream:
                for line in stream:
                    total += 1
                    row = json.loads(line)
                    if row.get("kind") == "decision" and row.get("surface") == SURFACE:
                        answer = row["answer"]
                        if "rationale" in answer:
                            if "rationale" in row and row["rationale"] != answer["rationale"]:
                                raise ValueError(f"conflicting rationale at line {total}")
                            row["rationale"] = answer["rationale"]
                            row["answer"] = {"result": answer["result"]}
                            changed += 1
                    elif row.get("kind") == "outcome" and row.get("decision_id") in qa_ids:
                        if isinstance(row.get("label"), str) and row["label"] in RESULTS:
                            row["label"] = {"result": row["label"]}
                            changed += 1
                    target.write(json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n")
        os.replace(temporary_name, output)
    finally:
        if temporary_name and os.path.exists(temporary_name):
            os.unlink(temporary_name)
    return total, changed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", required=True, type=Path, help="source JSONL ledger (read only)")
    parser.add_argument("--output", required=True, type=Path, help="new temporary JSONL ledger")
    args = parser.parse_args(argv)
    try:
        total, changed = normalize_ledger(args.ledger, args.output)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        parser.error(str(error))
    print(f"normalized {changed} of {total} rows into {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
