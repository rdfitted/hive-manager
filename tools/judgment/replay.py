#!/usr/bin/env python3
"""Replay frozen typed-QA evidence through a judge plugin without its verdict.

Plugins are callables addressed as ``module:function``. They receive canonical
UTF-8 JSON bytes containing only the saved observations, plus the sampling
configuration as a keyword argument. The built-in ``code-test`` plugin is a
deterministic plumbing check, not a QA judge.
"""

from __future__ import annotations

import argparse
import importlib
import json
import sys
from pathlib import Path
from typing import Any, Callable

import ledger
import retrieval_replay


JudgePlugin = Callable[..., Any]
FORBIDDEN_OBSERVATION_KEYS = {"answer", "result", "rationale"}
SOURCE_SURFACE = "hive.qa.criterion"


def code_test_judge(request: bytes, *, sampling: dict | None) -> dict:
    """A deterministic test double; it makes no claim about QA correctness."""
    observations = json.loads(request)
    return {"result": "pass" if observations.get("evidence") else "fail"}


def load_plugin(spec: str) -> JudgePlugin:
    if spec == "code-test":
        return code_test_judge
    module_name, separator, function_name = spec.partition(":")
    if not separator or not module_name or not function_name:
        raise ValueError("plugin must be code-test or module:function")
    plugin = getattr(importlib.import_module(module_name), function_name)
    if not callable(plugin):
        raise ValueError("plugin target is not callable")
    return plugin


def source_decisions(ledger_path: Path) -> list[dict]:
    """Read incumbent typed-QA decisions, excluding prior replay rows."""
    return sorted(
        (
            row
            for row in ledger.read_records([ledger_path])
            if row.get("kind") == "decision"
            and row.get("surface") == SOURCE_SURFACE
            and row.get("judge") == "incumbent-llm"
            and "source_decision_id" not in row
            and row.get("state_ref")
        ),
        key=lambda row: str(row["decision_id"]),
    )


def load_observations(row: dict, ledger_path: Path) -> Any:
    """Load only the frozen evidence file and check its recorded hash."""
    state_ref = row["state_ref"]
    source_id = str(row["decision_id"])
    if state_ref != f"evidence/{source_id}.json":
        raise ValueError(f"invalid state_ref for {source_id}")
    observations = ledger.load_evidence(state_ref, ledger_path)
    if row.get("state_hash") != ledger.sha256_of(observations):
        raise ValueError(f"evidence hash mismatch for {source_id}")
    return observations


def _assert_blind(value: Any) -> None:
    if isinstance(value, dict):
        leaked = FORBIDDEN_OBSERVATION_KEYS.intersection(value)
        if leaked:
            raise ValueError(f"evidence contains verdict field: {sorted(leaked)[0]}")
        for child in value.values():
            _assert_blind(child)
    elif isinstance(value, list):
        for child in value:
            _assert_blind(child)


def request_bytes(observations: Any) -> bytes:
    """Encode the blind observations once; every repeat gets these same bytes."""
    _assert_blind(observations)
    return ledger.canonical_json(observations).encode("utf-8")


def replay(
    ledger_path: Path,
    *,
    plugin: JudgePlugin,
    plugin_id: str,
    judge: str,
    runs: int,
    sampling: dict | None,
    model: str | None = None,
) -> int:
    if runs < 1:
        raise ValueError("runs must be positive")
    if judge not in ledger.JUDGES:
        raise ValueError(f"unsupported judge: {judge}")
    if sampling is not None and not isinstance(sampling, dict):
        raise ValueError("sampling must be a JSON object or None")
    sampling_json = ledger.canonical_json(sampling)
    existing, _ = retrieval_replay._existing_ledger_keys(ledger_path)
    written = 0
    for source in source_decisions(ledger_path):
        source_id = str(source["decision_id"])
        observations = load_observations(source, ledger_path)
        request = request_bytes(observations)
        for repeat in range(runs):
            decision_id = retrieval_replay._decision_id(
                "qa-replay", source_id, plugin_id, judge, model or "",
                sampling_json, str(repeat),
            )
            if decision_id in existing:
                continue
            run_sampling = json.loads(sampling_json)
            try:
                answer = plugin(request, sampling=run_sampling)
                error = None
            except Exception as exc:
                answer = None
                error = f"judge-error:{type(exc).__name__}"
            row = ledger.record_decision(
                source["surface"],
                source["subject_ref"],
                judge,
                answer,
                state_hash=source["state_hash"],
                state_ref=source["state_ref"],
                question_id=source.get("question_id"),
                question_version=source.get("question_version"),
                model=model if model is not None else plugin_id,
                sampling=json.loads(sampling_json),
                mode="shadow",
                error=error,
                decision_id=decision_id,
                ledger=ledger_path,
                **{"source_decision_id": source_id, "plugin_id": plugin_id},
            )
            if row is None:
                raise RuntimeError(f"replay row was not recorded for {source_id}")
            existing.add(decision_id)
            written += 1
    return written


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, help="judgment ledger JSONL path")
    parser.add_argument("--plugin", default="code-test", help="code-test or module:function")
    parser.add_argument("--judge", choices=sorted(ledger.JUDGES), help="ledger judge ID")
    parser.add_argument("--model", help="model ID for a model-backed plugin")
    parser.add_argument("--sampling", help="JSON object describing sampling")
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args(argv)
    if args.plugin != "code-test" and args.judge is None:
        parser.error("--judge is required for an external plugin")
    if args.plugin == "code-test" and args.judge not in (None, "code"):
        parser.error("code-test must record as judge=code")
    sampling = {"deterministic": True} if args.plugin == "code-test" else None
    if args.sampling is not None:
        try:
            sampling = json.loads(args.sampling)
        except json.JSONDecodeError as exc:
            parser.error(f"invalid --sampling JSON: {exc}")
        if not isinstance(sampling, dict):
            parser.error("--sampling must be a JSON object")
    try:
        ledger_path = retrieval_replay.resolve_ledger_path(args.ledger).resolve()
        written = replay(
            ledger_path,
            plugin=load_plugin(args.plugin),
            plugin_id=args.plugin,
            judge=args.judge or "code",
            runs=args.runs,
            sampling=sampling,
            model=args.model,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"replay: {exc}", file=sys.stderr)
        return 1
    print(f"wrote {written} replay decisions to {ledger_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
