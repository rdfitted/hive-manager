#!/usr/bin/env python3
"""Label typed-QA evidence in resumable, time-boxed sittings.

Only incumbent decisions are offered. A saved ``human-label`` outcome makes
the item disappear on the next run, including after Ctrl-C or an EOF.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import time
from pathlib import Path
from typing import Callable, TextIO

import audit
import ledger
import replay
import retrieval_replay


def _criterion_result(value):
    if isinstance(value, str) and value in {"pass", "fail", "blocked"}:
        return value
    if isinstance(value, dict) and len(value) == 1:
        key, number = next(iter(value.items()))
        if key in {"scored", "measured"} and isinstance(number, (int, float)) \
                and not isinstance(number, bool):
            try:
                if math.isfinite(number):
                    return {key: number}
            except OverflowError:
                pass
    raise ValueError("expected pass, fail, blocked, or a finite scored/measured value")


def _label_value(text: str):
    if text.lower() in {"pass", "fail", "blocked"}:
        return {"result": text.lower()}
    scored = re.fullmatch(r"scored\s+(.+)", text, flags=re.IGNORECASE)
    if scored:
        try:
            return {"result": _criterion_result({"scored": json.loads(scored.group(1))})}
        except json.JSONDecodeError as exc:
            raise ValueError("scored requires a JSON number") from exc
    try:
        value = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ValueError("invalid criterion result") from exc
    if isinstance(value, dict) and set(value) == {"result"}:
        value = value["result"]
    return {"result": _criterion_result(value)}


def label_session(
    ledger_path: Path,
    *,
    minutes: float = 10,
    split: str = "all",
    input_fn: Callable[[str], str] = input,
    output: TextIO = sys.stdout,
    clock: Callable[[], float] = time.monotonic,
) -> int:
    if minutes <= 0:
        raise ValueError("minutes must be positive")
    if split not in {"all", "tune", "heldout"}:
        raise ValueError("split must be all, tune, or heldout")
    _, existing_outcomes = retrieval_replay._existing_ledger_keys(ledger_path)
    labeled = {
        decision_id
        for decision_id, _, source in existing_outcomes
        if source == "human-label"
    }
    deadline = clock() + minutes * 60
    written = 0
    for row in replay.source_decisions(ledger_path):
        if clock() >= deadline:
            break
        source_id = str(row["decision_id"])
        if source_id in labeled:
            continue
        observations = replay.load_observations(row, ledger_path)
        heldout = audit.is_heldout(str(row["state_hash"]))
        item_split = "heldout" if heldout else "tune"
        if split != "all" and split != item_split:
            continue
        print(f"\n{source_id} [{item_split}]", file=output)
        print(replay.request_bytes(observations).decode("utf-8"), file=output)
        while True:
            try:
                response = input_fn("label (pass/fail/blocked, scored N, or JSON; skip/quit): ").strip()
            except (EOFError, KeyboardInterrupt):
                return written
            if response.lower() in {"quit", "q"}:
                return written
            if response.lower() in {"skip", "s", ""}:
                break
            try:
                value = _label_value(response)
            except ValueError as exc:
                print(f"Invalid label: {exc}", file=output)
                if clock() >= deadline:
                    return written
                continue
            break
        if response.lower() in {"skip", "s", ""}:
            continue
        saved = ledger.record_outcome(
            source_id,
            value,
            "human-label",
            ledger=ledger_path,
        )
        if saved is None:
            raise RuntimeError(f"label was not recorded for {source_id}")
        labeled.add(source_id)
        written += 1
    return written


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, help="judgment ledger JSONL path")
    parser.add_argument("--minutes", type=float, default=10)
    parser.add_argument("--split", choices=("all", "tune", "heldout"), default="all")
    args = parser.parse_args(argv)
    try:
        ledger_path = retrieval_replay.resolve_ledger_path(args.ledger).resolve()
        written = label_session(ledger_path, minutes=args.minutes, split=args.split)
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"label: {exc}", file=sys.stderr)
        return 1
    print(f"wrote {written} human labels to {ledger_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
