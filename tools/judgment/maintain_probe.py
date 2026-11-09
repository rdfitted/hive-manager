#!/usr/bin/env python3
"""Probe: can a typed judgment model separate maintainable functions from poor ones?

Asks the `code.maintain` questions about each labelled fixture function, several
times, and reports per question how well the answers separate the labelled-good
from the labelled-poor functions and how stable the decision is across runs.

This is an experiment, not a gate. It writes a report and changes nothing else.

    python tools/judgment/maintain_probe.py run --out report.json
    python tools/judgment/maintain_probe.py report report.json
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
from pathlib import Path

import jev_transport

HERE = Path(__file__).resolve().parent
SPEC_PATH = HERE / "code_maintain_questions.json"
FIXTURES_PATH = HERE / "fixtures" / "code-maintain" / "fixtures.json"
RUNS = 3


def api_key() -> str:
    key = os.environ.get("TYPESAFE_API_KEY", "")
    env_file = Path.home() / ".ai-gateway.env"
    if not key and env_file.is_file():
        for line in env_file.read_text(encoding="utf-8").splitlines():
            if line.startswith("TYPESAFE_API_KEY="):
                key = line.split("=", 1)[1].strip().strip('"')
    if not key:
        raise SystemExit("TYPESAFE_API_KEY is not set")
    return key


def value_of(answer: dict, levels: int | None) -> float:
    """One comparable number per answer, 0 to 1, higher meaning better code."""
    if answer["type"] == "noul":
        return float(answer["noul"])
    return float(answer["score"]) / (levels - 1)


def auc(good: list[float], poor: list[float]) -> float | None:
    """Chance a labelled-good function outscores a labelled-poor one. 0.5 is no separation."""
    if not good or not poor:
        return None
    wins = sum((g > p) + 0.5 * (g == p) for g in good for p in poor)
    return wins / (len(good) * len(poor))


def run(out: Path) -> dict:
    spec = json.loads(SPEC_PATH.read_text(encoding="utf-8"))
    fixtures = json.loads(FIXTURES_PATH.read_text(encoding="utf-8"))
    key = api_key()
    results = {"model_requested": spec["model"], "models_answering": [], "runs": RUNS, "items": {}}
    for item in fixtures["functions"]:
        answers = []
        for _ in range(RUNS):
            payload = {"model": spec["model"],
                       "state": {"language": item["language"], "function": item["code"]},
                       "questions": spec["questions"]}
            response = jev_transport.send(json.dumps(payload).encode("utf-8"), key)
            if response.get("model") not in results["models_answering"]:
                results["models_answering"].append(response.get("model"))
            answers.append(response["answers"])
        results["items"][item["id"]] = {"group": item["group"], "labels": item["labels"], "answers": answers}
        print(f"asked {item['id']}", file=sys.stderr)
    out.write_text(json.dumps(results, indent=1), encoding="utf-8")
    return results


def summarize(results: dict) -> dict:
    spec = json.loads(SPEC_PATH.read_text(encoding="utf-8"))
    # Which label each question is judged against, and whether a high answer means good.
    plan = {"single_job": ("single_job", True), "more_than_one_job": ("single_job", False),
            "naming": ("naming", True), "name_covers_behavior": ("naming", True),
            "local_reasoning": ("local_reasoning", True)}
    summary = {}
    for qid, (label, high_is_good) in plan.items():
        question = spec["questions"][qid]
        levels = len(question["criteria"]) if question["type"] == "score" else None
        good, poor, spreads, flips, by_group = [], [], [], 0, {}
        for item in results["items"].values():
            values = [value_of(run[qid], levels) for run in item["answers"]]
            values = values if high_is_good else [1 - v for v in values]
            mean = statistics.fmean(values)
            spreads.append(max(values) - min(values))
            flips += len({v >= 0.5 for v in values}) > 1
            by_group.setdefault(item["group"], []).append(mean)
            verdict = item["labels"].get(label)
            if verdict == "good":
                good.append(mean)
            elif verdict == "poor":
                poor.append(mean)
        summary[qid] = {
            "judged_against": label, "labelled_good": len(good), "labelled_poor": len(poor),
            "mean_good": round(statistics.fmean(good), 3) if good else None,
            "mean_poor": round(statistics.fmean(poor), 3) if poor else None,
            "auc": round(auc(good, poor), 3) if auc(good, poor) is not None else None,
            "max_run_spread": round(max(spreads), 3), "decision_flips": flips,
            "mean_by_group": {g: round(statistics.fmean(v), 3) for g, v in sorted(by_group.items())},
        }
    return summary


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="command", required=True)
    sub.add_parser("run").add_argument("--out", required=True)
    sub.add_parser("report").add_argument("results")
    a = ap.parse_args(argv)
    results = run(Path(a.out)) if a.command == "run" else json.loads(Path(a.results).read_text(encoding="utf-8"))
    print(json.dumps({"models_answering": results["models_answering"], "questions": summarize(results)}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
