#!/usr/bin/env python3
"""Ask the two-sided hive.qa.criterion questions over a frozen item file.

Shadow only and offline from any session: it writes ledger rows and nothing reads
them to route or gate. Every request goes through judge.py's gates (surface and
project allowlist, fail-closed redaction, size cap, ledgered key-free rows).

Items come from a file, not the ledger, on purpose. Today's incumbent rows hold
the Evaluator's own prose in `evidence` (#293), and sending that would show Jev
the Evaluator's reasoning. Ledger-sourced runs wait on QA-worker excerpts there.

Each item is {id, criterion: {number, description, kind}, evidence, evidence_refs};
any other field, such as a label, stays local. Only pass_fail criteria are sent.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import re
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Callable

import judge
import ledger


SURFACE = "hive.qa.criterion"
HERE = Path(__file__).resolve().parent
DEFAULT_QUESTIONS = HERE / "qa_two_sided_questions.json"
# Run limits mirror #295: at most 200 requests, backoff 5 s initial, 180 s total.
MAX_REQUESTS = 200
BACKOFF_INITIAL_S = 5.0
BACKOFF_TOTAL_S = 180.0
FINAL_STATUSES = {"egress-denied", "redaction-blocked", "size-blocked", "no-key", "invalid-input"}
# judge.py's blind check covers structured keys. This covers conclusions in text.
VERDICT_TEXT = re.compile(
    r"\b(?:PASS|PASSED|FAIL|FAILED|BLOCKED|QA_VERDICT)\b"
    r"|\b[Vv]erdict\b|\b[Rr]ationale\b"
    r"|\bCRITERION \d+:"
    r"|(?i:\b(?:criterion|requirement)\s+(?:is\s+|was\s+)?(?:met|satisfied|passes|passed|fails|failed)\b)"
)


class ItemError(ValueError):
    pass


def observations_of(item: Any) -> dict:
    """The blind state: an explicit allowlist, so a label can never travel with it."""
    if not isinstance(item, dict) or not isinstance(item.get("id"), str) or not item["id"]:
        raise ItemError("item needs an id")
    criterion = item.get("criterion")
    if not isinstance(criterion, dict) or not isinstance(criterion.get("description"), str):
        raise ItemError(f"{item['id']}: criterion needs a description")
    evidence = item.get("evidence")
    if not isinstance(evidence, str):
        raise ItemError(f"{item['id']}: evidence must be text")
    return {
        "criterion": {
            "number": criterion.get("number"),
            "description": criterion["description"],
            "kind": criterion.get("kind"),
        },
        "evidence": evidence,
        "evidence_refs": item.get("evidence_refs") or [],
    }


def skip_reason(item: dict, observations: dict) -> str | None:
    if item["criterion"].get("kind") != "pass_fail":
        return "not-pass-fail"
    text = ledger.canonical_json(observations)
    return "verdict-text" if VERDICT_TEXT.search(text) else None


def _completed_runs(ledger_path: Path, questions: Path) -> dict[str, int]:
    """Runs per item under the current wording. Another baseline's runs don't count."""
    done: dict[str, int] = {}
    if not ledger_path.exists():
        return done
    current = {question_id: ledger.sha256_of(question)
               for question_id, question in json.loads(questions.read_text(encoding="utf-8")).items()}
    requests: dict[str, dict] = {}
    for row in ledger.read_records([ledger_path]):
        if (row.get("kind") == "decision" and row.get("judge") == "jev" and row.get("error") is None
                and row.get("request_id") and row.get("source_decision_id")):
            request = requests.setdefault(str(row["request_id"]), {"source": str(row["source_decision_id"]), "asked": {}})
            request["asked"][row.get("question_id")] = row.get("question_version")
    for request in requests.values():
        if request["asked"] == current:
            done[request["source"]] = done.get(request["source"], 0) + 1
    return done


def _ask(item_id: str, subject: dict, observations: dict, questions: Path, ledger_path: Path,
         policy: Path | None, dictionary: str | None, dry_run: bool) -> dict:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        subject_path = root / "subject.json"
        subject_path.write_text(json.dumps(subject), encoding="utf-8")
        state_path = root / "observations.json"
        state_path.write_text(json.dumps(observations), encoding="utf-8")
        argv = ["--ledger", str(ledger_path)]
        if policy is not None:
            argv += ["--policy", str(policy)]
        if dictionary is not None:
            argv += ["--redact-dictionary", dictionary]
        argv += ["ask", "--surface", SURFACE, "--subject-ref", str(subject_path),
                 "--observations", str(state_path), "--questions", str(questions),
                 "--source-decision-id", item_id]
        if dry_run:
            argv.append("--dry-run")
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            judge.main(argv)
    return json.loads(output.getvalue())


def run(
    items: list[dict],
    ledger_path: Path,
    *,
    questions: Path = DEFAULT_QUESTIONS,
    runs: int = 3,
    max_requests: int = MAX_REQUESTS,
    policy: Path | None = None,
    dictionary: str | None = None,
    dry_run: bool = False,
    sleep: Callable[[float], None] = time.sleep,
) -> dict:
    """Send up to max_requests, resuming past completed runs. Returns a key-free summary."""
    if runs < 1:
        raise ValueError("runs must be positive")
    prepared = []
    skipped: dict[str, str] = {}
    for item in items:
        observations = observations_of(item)
        reason = skip_reason(item, observations)
        if reason:
            skipped[item["id"]] = reason
        else:
            prepared.append((item["id"], {"qa_bench_item": item["id"]}, observations))
    summary: dict[str, Any] = {
        "requests": 0, "recorded": 0, "statuses": {}, "skipped": skipped, "stopped": None,
    }
    done = {} if dry_run else _completed_runs(ledger_path, questions)
    # One full pass at a time, so a capped or interrupted run stays comparable per pass.
    for pass_index in range(runs):
        for item_id, subject, observations in prepared:
            if done.get(item_id, 0) > pass_index:
                continue
            waited = 0.0
            delay = BACKOFF_INITIAL_S
            while True:
                if summary["requests"] >= max_requests:
                    summary["stopped"] = "max-requests"
                    return summary
                result = _ask(item_id, subject, observations, questions, ledger_path,
                              policy, dictionary, dry_run)
                status = result["status"]
                summary["requests"] += 1
                summary["statuses"][status] = summary["statuses"].get(status, 0) + 1
                if status == "recorded":
                    summary["recorded"] += 1
                    done[item_id] = done.get(item_id, 0) + 1
                    break
                if status in FINAL_STATUSES or status == "dry-run":
                    if status in FINAL_STATUSES:
                        summary["skipped"][item_id] = status
                    break
                remaining = BACKOFF_TOTAL_S - waited
                if remaining <= 0:
                    summary["stopped"] = "backoff-exhausted"
                    return summary
                pause = min(delay, remaining)
                sleep(pause)
                waited += pause
                delay *= 2
    return summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--items", required=True, type=Path, help="JSON file with an items list")
    parser.add_argument("--ledger", required=True, type=Path,
                        help="a scratch ledger; the default production ledger is refused")
    parser.add_argument("--questions", type=Path, default=DEFAULT_QUESTIONS)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--max-requests", type=int, default=MAX_REQUESTS)
    parser.add_argument("--policy", type=Path)
    parser.add_argument("--redact-dictionary")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    try:
        ledger_path = args.ledger.resolve()
        policy = args.policy or HERE / "egress-policy.json"
        if ledger_path == judge._default_ledger(policy).resolve():
            raise ValueError("refusing the default ledger; bench rows would mix with live metrics")
        loaded = json.loads(args.items.read_text(encoding="utf-8"))
        items = loaded.get("items") if isinstance(loaded, dict) else None
        if not isinstance(items, list):
            raise ValueError("items file needs an items list")
        summary = run(items, ledger_path, questions=args.questions, runs=args.runs,
                      max_requests=args.max_requests, policy=args.policy,
                      dictionary=args.redact_dictionary, dry_run=args.dry_run)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"shadow_qa: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
