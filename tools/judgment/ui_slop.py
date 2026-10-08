#!/usr/bin/env python3
"""Blind ui.slop shadow asks and per-question local reporting. Never changes a gate."""
from __future__ import annotations
import argparse
import io
import json
import math
import tempfile
from contextlib import redirect_stdout
from pathlib import Path, PureWindowsPath
from typing import Any
import judge
import ledger

SPEC_PATH = Path(__file__).with_name("ui_slop_questions.json")


def relative_path(value: Any) -> str:
    if (not isinstance(value, str) or not value or "\\" in value
            or value.startswith("/") or PureWindowsPath(value).drive
            or ":" in value or any(p in {"", ".", ".."} for p in value.split("/"))):
        raise judge.InputError("finding needs a repository-relative relpath")
    return value


def observations(finding: dict, page_kind: str, *, ignored: bool) -> dict:
    relpath = relative_path(finding.get("relpath"))
    fields = {"rule": {"id": finding.get("antipattern", ""),
                       "name": finding.get("name", "")},
              "relpath": relpath, "snippet": finding.get("snippet", ""),
              "context": finding.get("context", ""), "page_kind": page_kind}
    if ignored:
        fields["ignore_reason"] = finding.get("reason", "")
    if not all(isinstance(v, str) for v in [*fields["rule"].values(),
                 fields["snippet"], fields["context"], fields["page_kind"],
                 fields.get("ignore_reason", "")]):
        raise judge.InputError("finding text must contain strings")
    fields["context"] = fields["context"][:300]
    judge._blind(fields)
    return fields


def local_labels(path: Path | None) -> dict:
    """Labels remain local and are never part of the blind request."""
    labels = {}
    if path:
        for line in path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if (not isinstance(row, dict) or not isinstance(row.get("finding_key"), str)
                    or not isinstance(row.get("question_id"), str)
                    or not isinstance(row.get("label"), bool)):
                raise judge.InputError("labels need finding_key, question_id and boolean label")
            labels[(row["finding_key"], row["question_id"])] = row["label"]
    return labels


def ask_verdict(verdict_path: Path, *, ledger_path: Path, dictionary: str,
                policy: Path | None = None, dry_run: bool = False,
                labels_path: Path | None = None, page_kind: str = "unknown") -> dict:
    verdict = judge._read_json(str(verdict_path))
    if not isinstance(verdict, dict) or verdict.get("verdict_version") != 2:
        raise judge.InputError("requires verdict v2")
    spec = judge._read_json(str(SPEC_PATH))
    labels = local_labels(labels_path)
    prepared = []
    for bucket, question_id in (("gating", "matches_rule"), ("ignored", "ignore_reason_specific")):
        findings = verdict.get(bucket, [])
        if not isinstance(findings, list):
            raise judge.InputError("finding bucket must be a list")
        for finding in findings:
            if not isinstance(finding, dict):
                raise judge.InputError("finding must be an object")
            state = observations(finding, page_kind, ignored=bucket == "ignored")
            key = finding.get("finding_key")
            if not isinstance(key, str) or not key:
                raise judge.InputError("finding needs finding_key")
            questions = {qid: spec["questions"][qid]["question"]
                         for qid in (question_id, "injection")
                         if spec["questions"][qid]["wired"]}
            prepared.append((key, state, questions))
    results = []
    # Preflight every finding before calling judge; no output is written to verdict_path.
    with tempfile.TemporaryDirectory(prefix="ui-slop-shadow-") as temporary:
        root = Path(temporary)
        for key, state, questions in prepared:
            for name, value in (("subject", {"finding_key": key, "relpath": state["relpath"]}),
                                ("state", state), ("questions", questions)):
                (root / (name + ".json")).write_text(ledger.canonical_json(value), encoding="utf-8")
            argv = ["--ledger", str(ledger_path), "--redact-dictionary", dictionary]
            if policy:
                argv += ["--policy", str(policy)]
            argv += ["ask", "--surface", "ui.slop", "--subject-ref", str(root / "subject.json"),
                     "--observations", str(root / "state.json"),
                     "--questions", str(root / "questions.json")]
            if dry_run:
                argv.append("--dry-run")
            output = io.StringIO()
            with redirect_stdout(output):
                code = judge.main(argv)
            result = json.loads(output.getvalue())
            results.append({"finding_key": key, "exit_code": code, "shadow": result})
            for qid, decision in result.get("questions", {}).items():
                label = labels.get((key, qid))
                if label is not None and decision["status"] == "recorded":
                    row = ledger.record_outcome(decision["decision_id"],
                        {"result": "true" if label else "false"}, "human-label", ledger=ledger_path)
                    if row is None:
                        raise judge.InputError("local label could not be recorded")
    return {"mode": "shadow", "routed": "none", "findings": results}


def report(ledger_path: Path) -> dict:
    rows = list(ledger.read_records([ledger_path]))
    labels = {r["decision_id"]: r["label"] for r in rows if r.get("kind") == "outcome"
              and r.get("source") == "human-label"}
    groups = {}
    for row in rows:
        if row.get("kind") != "decision" or row.get("surface") != "ui.slop":
            continue
        group = groups.setdefault(row["question_id"], {"rows": 0, "statuses": {},
            "bands": {"act": 0, "human": 0, "drop": 0}, "labeled": 0,
            "brier_sum": 0.0})
        group["rows"] += 1
        status = row.get("error") or "recorded"
        group["statuses"][status] = group["statuses"].get(status, 0) + 1
        p = row.get("noul")
        if isinstance(p, (int, float)) and not isinstance(p, bool) and math.isfinite(p) and 0 <= p <= 1:
            answer = row.get("answer")
            band = answer.get("result") if isinstance(answer, dict) else None
            if not row.get("error") and isinstance(band, str) and band in group["bands"]:
                group["bands"][band] += 1
            label = labels.get(row["decision_id"], {}).get("result")
            if label in {"true", "false"}:
                group["labeled"] += 1
                group["brier_sum"] += (p - (label == "true")) ** 2
    for group in groups.values():
        total = group.pop("brier_sum")
        group["brier_score"] = total / group["labeled"] if group["labeled"] else None
    return {"surface": "ui.slop", "mode": "shadow", "questions": groups}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    ask = commands.add_parser("ask")
    ask.add_argument("--verdict", required=True, type=Path)
    ask.add_argument("--ledger", required=True, type=Path)
    ask.add_argument("--redact-dictionary", required=True)
    ask.add_argument("--policy", type=Path)
    ask.add_argument("--labels", type=Path, help="local JSONL: finding_key, question_id, boolean label")
    ask.add_argument("--page-kind", default="unknown")
    ask.add_argument("--dry-run", action="store_true")
    summary = commands.add_parser("report")
    summary.add_argument("--ledger", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "report":
            value = report(args.ledger)
        else:
            value = ask_verdict(args.verdict, ledger_path=args.ledger,
                dictionary=args.redact_dictionary, policy=args.policy, dry_run=args.dry_run,
                labels_path=args.labels, page_kind=args.page_kind)
        print(ledger.canonical_json(value))
        return 0
    except (judge.InputError, OSError, UnicodeError, ValueError, TypeError, KeyError):
        print(ledger.canonical_json({"status": "invalid-input", "mode": "shadow"}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
