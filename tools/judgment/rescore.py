#!/usr/bin/env python3
"""Recompute recorded Jev judgments under other rules, with no model call.

The ledger is the trace: each row keeps the state hash, question version, model,
raw probabilities and the outcome join. So a threshold or combination rule can be
tested without API spend or Jev's run-to-run jitter. Nothing here imports a
transport, and the ledger is only read. replay.py is the other direction: it calls
a judge again over frozen evidence.

Results are split by baseline (question versions). Baselines are never compared
with each other, and a report without enough human labels says it's too thin.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

import audit
import calibration
import ledger


QA_SURFACE = "hive.qa.criterion"
MISSES_SURFACE = "hive.qa.criterion.misses"
INJECTION_SURFACE = "hive.qa.criterion.injection"
REVIEW_SURFACE = "hive.review.finding"
PASS_RESULTS = {"pass", "fail"}


def _probability(row: dict) -> float | None:
    value = row.get("noul")
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        return None
    return float(value)


def combined(rule: dict, meets: float, misses: float) -> float:
    if rule["combine"] == "meets":
        return meets
    if rule["combine"] == "min_meets_not_misses":
        return round(min(meets, 1.0 - misses), 6)
    raise ValueError(f"unknown combine rule: {rule['combine']}")


def decide(rule: dict, probability: float) -> str:
    if probability >= rule["pass_min"]:
        return "pass"
    if "fail_below" in rule and probability >= rule["fail_below"]:
        return "abstain"
    return "fail"


def _rate(k: int, n: int) -> dict:
    interval = calibration.wilson(k, n)
    return {"k": k, "n": n, "rate": k / n if n else None,
            "wilson_95": list(interval) if interval else None}


def score(rule: dict, items: list[dict]) -> dict:
    """items: {probability, truth}, one per judged item."""
    false_pass = false_fail = true_pass = true_fail = abstain = 0
    pairs = []
    for item in items:
        decision = decide(rule, item["probability"])
        if decision == "abstain":
            abstain += 1
            continue
        if decision == "pass":
            true_pass += item["truth"] == "pass"
            false_pass += item["truth"] == "fail"
        else:
            true_fail += item["truth"] == "fail"
            false_fail += item["truth"] == "pass"
        _, confidence = calibration.noul_to_pick(item["probability"], 0.5)
        pairs.append((confidence, decision == item["truth"]))
    decided = len(items) - abstain
    right = true_pass + true_fail
    # ECE is defined on the 0.5 pick, so a band rule doesn't report its own.
    ece, reliability = (None, None)
    if rule["pass_min"] == 0.5 and "fail_below" not in rule:
        ece, reliability = calibration.ece(pairs)
    return {
        "n": len(items), "decided": decided, "abstained": abstain,
        "false_passes": false_pass, "false_fails": false_fail,
        "accuracy": _rate(right, decided),
        "false_pass_rate": _rate(false_pass, false_pass + true_fail),
        "false_fail_rate": _rate(false_fail, false_fail + true_pass),
        "ece": ece, "reliability": reliability,
    }


def paired(rule_a: dict, rule_b: dict, items: list[dict]) -> dict:
    """Exact sign test on the items where exactly one of two rules is right."""
    a_only = b_only = 0
    for item in items:
        a = decide(rule_a, item["probability_by_rule"][rule_a["id"]]) == item["truth"]
        b = decide(rule_b, item["probability_by_rule"][rule_b["id"]]) == item["truth"]
        a_only += a and not b
        b_only += b and not a
    n = a_only + b_only
    p = None
    if n:
        tail = sum(math.comb(n, k) for k in range(min(a_only, b_only) + 1)) / 2 ** n
        p = min(1.0, 2 * tail)
    return {"a": rule_a["id"], "b": rule_b["id"], "a_right_b_wrong": a_only,
            "b_right_a_wrong": b_only, "sign_test_p": p}


def repeatability(rule: dict, runs_by_item: dict[str, list[float]]) -> dict:
    """Judged on the decision. Probability spread is jitter, reported apart."""
    groups = flips = 0
    spreads = []
    for values in runs_by_item.values():
        if len(values) < 2:
            continue
        groups += 1
        spreads.append(max(values) - min(values))
        flips += len({decide(rule, value) for value in values}) > 1
    p95 = calibration.percentile(spreads, 95)
    return {"items_repeated": groups, "decision_flips": flips,
            "jitter_max": round(max(spreads), 6) if spreads else None,
            "jitter_p95": round(p95, 6) if p95 is not None else None}


def _truth_of(ids: list[str], outcomes: dict, labels: dict[str, str] | None,
              sources: set[str], excluded: dict[str, int]) -> str | None:
    """(pass|fail) from an allowed source. Author labels replace the outcome join."""
    if labels is not None:
        for identifier in ids:
            if identifier in labels:
                return labels[identifier]
        return None
    candidates = [row for identifier in ids for row in outcomes.get(identifier, [])]
    allowed = [row for row in candidates if row.get("source") in sources]
    for row in candidates:
        if row.get("source") not in sources:
            excluded[str(row.get("source"))] = excluded.get(str(row.get("source")), 0) + 1
    if not allowed:
        return None
    best = max(allowed, key=lambda row: (audit.AUTHORITY.get(row.get("source"), 0), row.get("ts", "")))
    label = best.get("label")
    value = label.get("result") if isinstance(label, dict) else label
    return value if isinstance(value, str) else None


def _check_evidence(row: dict, ledger_file: Path, tally: dict[str, int]) -> bool:
    """False only when the recorded evidence no longer rehashes to its state_hash."""
    state_ref = row.get("state_ref")
    if not state_ref or not (ledger_file.parent / state_ref).is_file():
        tally["not_available"] += 1
        return True
    if ledger.sha256_of(ledger.load_evidence(state_ref, ledger_file)) != row.get("state_hash"):
        tally["mismatch"] += 1
        return False
    tally["verified"] += 1
    return True


def _load(ledger_file: Path) -> tuple[list[dict], dict]:
    decisions, outcomes = [], defaultdict(list)
    for row in ledger.read_records([ledger_file]):
        if row.get("kind") == "decision":
            decisions.append(row)
        elif row.get("kind") == "outcome":
            outcomes[str(row.get("decision_id"))].append(row)
    return decisions, outcomes


def _qa_runs(decisions: list[dict], ledger_file: Path, counters: dict) -> list[dict]:
    """One run per fan-out request that recorded both halves with no error."""
    requests: dict[str, dict[str, dict]] = defaultdict(dict)
    for row in decisions:
        if row.get("judge") != "jev" or row.get("surface") not in (QA_SURFACE, MISSES_SURFACE, INJECTION_SURFACE):
            continue
        if row.get("error") is not None:
            counters["error_rows"] += 1
            continue
        request_id = row.get("request_id")
        if not request_id:
            counters["rows_without_request_id"] += row.get("surface") == QA_SURFACE
            continue
        requests[str(request_id)][row["surface"]] = row
    runs = []
    for request_id, parts in requests.items():
        meets_row, misses_row = parts.get(QA_SURFACE), parts.get(MISSES_SURFACE)
        if meets_row is None or misses_row is None:
            counters["incomplete_requests"] += 1
            continue
        meets, misses = _probability(meets_row), _probability(misses_row)
        if meets is None or misses is None:
            counters["incomplete_requests"] += 1
            continue
        if not _check_evidence(meets_row, ledger_file, counters["evidence"]):
            continue
        recorded = [(meets_row, "pass" if meets >= 0.5 else "fail"),
                    (misses_row, "fail" if misses >= 0.5 else "pass")]
        counters["fidelity_mismatches"] += sum(
            (row.get("answer") or {}).get("result") != expected for row, expected in recorded)
        injection_row = parts.get(INJECTION_SURFACE)
        runs.append({
            "request_id": request_id, "ts": meets_row.get("ts", ""),
            "source": meets_row.get("source_decision_id"),
            "state_hash": meets_row.get("state_hash"),
            "decision_id": meets_row["decision_id"],
            "model": meets_row.get("model"),
            "baseline": ledger.canonical_json([
                meets_row.get("question_version"), misses_row.get("question_version"),
                (injection_row or {}).get("question_version")]),
            "meets": meets, "misses": misses,
            "injection": _probability(injection_row) if injection_row else None,
        })
    return sorted(runs, key=lambda run: (run["ts"], run["request_id"]))


def _thin_reasons(truth_class: str, labeled: int, heldout: int, targets: dict) -> list[str]:
    reasons = []
    if truth_class != "ledger-outcomes":
        reasons.append(f"truth is {truth_class}, not human-confirmed labels on real QA work")
    if labeled < targets.get("min_labeled_n", 200):
        reasons.append(f"labeled n {labeled} < {targets.get('min_labeled_n', 200)}")
    if heldout < targets.get("min_heldout_n", 100):
        reasons.append(f"held-out n {heldout} < {targets.get('min_heldout_n', 100)}")
    return reasons


def rescore_qa(ledger_file: Path, tables: dict, labels: dict[str, str] | None = None,
               truth_sources: set[str] | None = None) -> dict:
    config = tables["rescore"][QA_SURFACE]
    rules = config["rules"]
    sources = truth_sources if truth_sources is not None else set(config["truth_sources"])
    decisions, outcomes = _load(ledger_file)
    counters: dict[str, Any] = {
        "error_rows": 0, "rows_without_request_id": 0, "incomplete_requests": 0,
        "fidelity_mismatches": 0, "evidence": {"verified": 0, "mismatch": 0, "not_available": 0},
        "excluded_outcomes_by_source": {}, "unlabeled_items": 0, "non_pass_fail_labels": 0,
    }
    runs = _qa_runs(decisions, ledger_file, counters)
    by_baseline: dict[str, list[dict]] = defaultdict(list)
    for run in runs:
        by_baseline[run["baseline"]].append(run)
    truth_class = "bench-author" if labels is not None else "ledger-outcomes"
    report = {
        "surface": QA_SURFACE, "truth_class": truth_class,
        "truth_sources": sorted(sources) if labels is None else None,
        "counters": counters, "baselines": [],
        "baselines_compared_with_each_other": False,
    }
    targets = tables.get("targets", {})
    for baseline, members in sorted(by_baseline.items()):
        report["baselines"].append(_baseline_report(
            baseline, members, rules, config.get("compare", []), outcomes, labels, sources,
            counters, truth_class, targets))
    return report


def _baseline_report(baseline: str, runs: list[dict], rules: list[dict], compare: list,
                     outcomes: dict, labels: dict[str, str] | None, sources: set[str],
                     counters: dict, truth_class: str, targets: dict) -> dict:
    by_item: dict[str, list[dict]] = defaultdict(list)
    for run in runs:
        by_item[str(run["source"] or run["state_hash"])].append(run)
    labeled = []
    for key, item_runs in by_item.items():
        first = item_runs[0]
        ids = [identifier for identifier in (first["source"], first["decision_id"]) if identifier]
        truth = _truth_of(ids, outcomes, labels, sources, counters["excluded_outcomes_by_source"])
        if truth is None:
            counters["unlabeled_items"] += 1
            continue
        if truth not in PASS_RESULTS:
            counters["non_pass_fail_labels"] += 1
            continue
        labeled.append({"key": key, "truth": truth, "runs": item_runs,
                        "heldout": audit.is_heldout(str(first["state_hash"]))})
    def view(rule_id_to_rule: dict, index: int, subset: list[dict]) -> list[dict]:
        rows = []
        for item in subset:
            if index >= len(item["runs"]):
                continue
            run = item["runs"][index]
            rows.append({
                "truth": item["truth"],
                "probability_by_rule": {rule_id: combined(rule, run["meets"], run["misses"])
                                        for rule_id, rule in rule_id_to_rule.items()},
            })
        return rows
    index_of = {rule["id"]: rule for rule in rules}
    splits = {"all": labeled, "tune": [i for i in labeled if not i["heldout"]],
              "heldout": [i for i in labeled if i["heldout"]]}
    scored: dict[str, dict] = {}
    for split, subset in splits.items():
        first_runs = view(index_of, 0, subset)
        scored[split] = {
            "rules": {rule["id"]: score(rule, [
                {"probability": row["probability_by_rule"][rule["id"]], "truth": row["truth"]}
                for row in first_runs]) for rule in rules},
            "paired": [paired(index_of[a], index_of[b], first_runs) for a, b in compare],
        }
    max_runs = max((len(item["runs"]) for item in labeled), default=0)
    per_pass = {}
    for rule in rules:
        per_pass[rule["id"]] = [
            {key: value for key, value in score(rule, [
                {"probability": row["probability_by_rule"][rule["id"]], "truth": row["truth"]}
                for row in view(index_of, index, labeled)]).items()
             if key in {"n", "abstained", "false_passes", "false_fails"}}
            for index in range(max_runs)
        ]
    repeated = {
        rule["id"]: repeatability(rule, {
            key: [combined(rule, run["meets"], run["misses"]) for run in item_runs]
            for key, item_runs in by_item.items()})
        for rule in rules
    }
    flagged = sorted({
        key for key, item_runs in by_item.items()
        if any(run["injection"] is not None and run["injection"] >= 0.5 for run in item_runs)
    })
    return {
        "question_versions": json.loads(baseline),
        "models": sorted({str(run["model"]) for run in runs}),
        "items": len(by_item), "runs": len(runs), "labeled_items": len(labeled),
        "too_thin_to_conclude": _thin_reasons(truth_class, len(labeled), len(splits["heldout"]), targets),
        "splits": scored, "per_pass_counts": per_pass, "repeatability": repeated,
        "injection_flagged_items": flagged,
    }


def rescore_review(ledger_file: Path, tables: dict) -> dict:
    """Recorded Choice rows: replay the top label, then test an abstain floor."""
    config = tables["rescore"][REVIEW_SURFACE]
    sources = set(config["truth_sources"])
    decisions, outcomes = _load(ledger_file)
    rows = [row for row in decisions if row.get("surface") == REVIEW_SURFACE
            and row.get("judge") == "jev" and row.get("error") is None
            and isinstance(row.get("probabilities"), dict) and row["probabilities"]]
    by_baseline: dict[str, list[dict]] = defaultdict(list)
    for row in rows:
        by_baseline[str(row.get("question_version"))].append(row)
    report = {"surface": REVIEW_SURFACE, "baselines": [], "baselines_compared_with_each_other": False}
    excluded: dict[str, int] = {}
    for baseline, members in sorted(by_baseline.items()):
        mismatches = 0
        joined = []
        for row in sorted(members, key=lambda item: item.get("ts", "")):
            probabilities = row["probabilities"]
            top = max(probabilities, key=probabilities.get)
            mismatches += probabilities[top] > probabilities.get((row.get("answer") or {}).get("result"), -1)
            ids = [i for i in (row.get("source_decision_id"), row["decision_id"]) if i]
            truth = _truth_of(ids, outcomes, None, sources, excluded)
            if truth is not None:
                joined.append((top, float(probabilities[top]), truth))
        results = {}
        for rule in config["rules"]:
            kept = [item for item in joined if item[1] >= rule["pass_min"]]
            right = sum(top == truth for top, _, truth in kept)
            results[rule["id"]] = {"decided": len(kept), "abstained": len(joined) - len(kept),
                                   "agreement": _rate(right, len(kept))}
        report["baselines"].append({
            "question_version": baseline, "rows": len(members), "labeled_rows": len(joined),
            "replay_mismatches": mismatches, "rules": results,
            "too_thin_to_conclude": _thin_reasons(
                "downstream outcomes", len(joined), 0, tables.get("targets", {})),
        })
    return report


def _pct(rate: dict) -> str:
    return "n/a" if rate["rate"] is None else f"{rate['rate'] * 100:.1f}% ({rate['k']}/{rate['n']})"


def render_qa(report: dict) -> str:
    counters = report["counters"]
    lines = [f"# Rescore: {report['surface']}", f"truth: {report['truth_class']}",
             f"counters: {json.dumps(counters, sort_keys=True)}", ""]
    if not report["baselines"]:
        lines.append("No paired two-sided runs in this ledger. Nothing to measure.")
    for baseline in report["baselines"]:
        lines += ["## Baseline " + " / ".join(str(v)[:15] for v in baseline["question_versions"]),
                  f"models {baseline['models']}; items {baseline['items']}, runs {baseline['runs']}, "
                  f"labeled {baseline['labeled_items']}"]
        for reason in baseline["too_thin_to_conclude"]:
            lines.append(f"- TOO THIN: {reason}")
        for split, content in baseline["splits"].items():
            lines += ["", f"### {split} (first pass per item)",
                      "| rule | n | abstain | false pass | false fail | accuracy | ECE |",
                      "|---|---|---|---|---|---|---|"]
            for rule_id, row in content["rules"].items():
                ece = "n/a" if row["ece"] is None else f"{row['ece']:.3f}"
                lines.append(f"| {rule_id} | {row['n']} | {row['abstained']} | {row['false_passes']} | "
                             f"{row['false_fails']} | {_pct(row['accuracy'])} | {ece} |")
            for pair in content["paired"]:
                p = "n/a" if pair["sign_test_p"] is None else f"{pair['sign_test_p']:.3f}"
                lines.append(f"\n{pair['a']} right, {pair['b']} wrong: {pair['a_right_b_wrong']}; "
                             f"the reverse: {pair['b_right_a_wrong']}; sign test p = {p}")
        lines += ["", "### Repeatability (decision flips; jitter apart)"]
        for rule_id, row in baseline["repeatability"].items():
            lines.append(f"- {rule_id}: {row['decision_flips']} flips over {row['items_repeated']} items, "
                         f"jitter max {row['jitter_max']}")
        lines.append(f"\ninjection flagged: {baseline['injection_flagged_items']}")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ledger", required=True, type=Path, help="ledger JSONL, read only")
    parser.add_argument("--surface", choices=[QA_SURFACE, REVIEW_SURFACE], default=QA_SURFACE)
    parser.add_argument("--tables", type=Path, default=Path(__file__).resolve().parent / "decision-tables.json")
    parser.add_argument("--rules", type=Path, help="JSON rules that replace the tables' rules for this run")
    parser.add_argument("--labels", type=Path, help="bench file; its labels are reported as bench-author truth")
    parser.add_argument("--json", type=Path, help="also write the full report here")
    args = parser.parse_args(argv)
    try:
        tables = json.loads(args.tables.read_text(encoding="utf-8"))
        if args.rules:
            tables["rescore"][args.surface]["rules"] = json.loads(args.rules.read_text(encoding="utf-8"))
        if args.surface == REVIEW_SURFACE:
            report = rescore_review(args.ledger, tables)
            text = json.dumps(report, indent=2, sort_keys=True)
        else:
            labels = None
            if args.labels:
                bench = json.loads(args.labels.read_text(encoding="utf-8"))
                labels = {item["id"]: item["label"] for item in bench["items"]}
            report = rescore_qa(args.ledger, tables, labels)
            text = render_qa(report)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"rescore: {exc}", file=sys.stderr)
        return 1
    if args.json:
        args.json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
