#!/usr/bin/env python3
"""Record shadow judgments and make locally gated, blind Jev requests.

ask: --surface --subject-ref JSON --observations JSON --question-id --question JSON
record: --surface --subject-ref JSON --answer JSON --question-id
outcome: --decision-id --label JSON --source

Path overrides: --ledger, --tables, --policy, --redact-dictionary (or the
JUDGMENT_LEDGER, JUDGMENT_TABLES, JUDGMENT_EGRESS_POLICY, and
JUDGMENT_REDACT_DICTIONARY environment variables). A dry run checks every
gate, writes a local row, and never calls the provider.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any

import jev_transport
import ledger
import redaction


HERE = Path(__file__).resolve().parent
FORBIDDEN = {"answer", "rationale", "severity", "verdict", "result", "measured"}
MAX_REQUEST_BYTES = 32 * 1024
QUESTION_TYPES = {"hive.qa.criterion": "noul", "hive.review.finding": "choice"}
GITHUB_REMOTE = re.compile(
    r"^(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)"
    r"([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?/?$",
    re.IGNORECASE,
)


class InputError(ValueError):
    pass


class JudgeParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        _emit(_result("invalid-input", "", error="invalid-input"), 2)
        raise SystemExit(2)


def _read_json(path: str) -> Any:
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise InputError("unreadable JSON input") from exc


def _blind(value: Any) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if not isinstance(key, str) or key.lower() in FORBIDDEN:
                raise InputError("blind payload contains a forbidden structured field")
            _blind(child)
    elif isinstance(value, list):
        for child in value:
            _blind(child)
    elif value is not None and not isinstance(value, (str, bool, int, float)):
        raise InputError("unsupported blind payload value")


def _project_identity(cwd: Path | None = None) -> str | None:
    """Use only this checkout's origin, never caller-supplied project data."""
    try:
        root = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"], cwd=cwd,
            capture_output=True, text=True, timeout=5, check=True,
        ).stdout.strip()
        if not root:
            return None
        remote = subprocess.run(
            ["git", "remote", "get-url", "origin"], cwd=root,
            capture_output=True, text=True, timeout=5, check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return None
    match = GITHUB_REMOTE.fullmatch(remote)
    return f"{match.group(1)}/{match.group(2)}".lower() if match else None


def _policy_entry(policy: Any, group: str, name: str | None) -> dict | None:
    if not isinstance(policy, dict) or policy.get("version") != 1 or policy.get("default") != "deny":
        return None
    entries = policy.get(group)
    entry = entries.get(name) if isinstance(entries, dict) and name else None
    if not isinstance(entry, dict) or entry.get("allow") is not True:
        return None
    if not isinstance(entry.get("decided"), str) or not entry["decided"]:
        return None
    if not isinstance(entry.get("by"), str) or not entry["by"]:
        return None
    if group == "surfaces" and entry.get("redact") != "names":
        return None
    return entry


def _policy(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, UnicodeError, json.JSONDecodeError):
        return {}


def _json_bytes(value: Any) -> bytes:
    return ledger.canonical_json(value).encode("utf-8")


def _question(surface: str, value: Any) -> dict:
    if surface not in QUESTION_TYPES or not isinstance(value, dict):
        raise InputError("unsupported surface or question")
    expected = QUESTION_TYPES[surface]
    if value.get("type") != expected or "instructions" not in value:
        raise InputError("question type or instructions missing")
    if expected == "choice":
        criteria = value.get("criteria")
        if not isinstance(criteria, dict) or set(criteria) != {"ACCEPT", "PARTIAL", "DECLINE"}:
            raise InputError("choice criteria must contain ACCEPT, PARTIAL, DECLINE")
    elif "criteria" in value:
        criteria = value["criteria"]
        if not isinstance(criteria, dict) or set(criteria) != {"true", "false"}:
            raise InputError("noul criteria must contain true and false")
    _blind(value)
    return value


def _answer(surface: str, response: Any, question_id: str) -> tuple[dict, dict]:
    if not isinstance(response, dict) or not isinstance(response.get("model"), str):
        raise jev_transport.TransportError("invalid-response")
    answers = response.get("answers")
    item = answers.get(question_id) if isinstance(answers, dict) else None
    if not isinstance(item, dict):
        raise jev_transport.TransportError("invalid-answer")
    if surface == "hive.qa.criterion":
        probability = item.get("noul")
        if item.get("type") != "noul" or isinstance(probability, bool) or not isinstance(probability, (int, float)) or not math.isfinite(probability) or not 0 <= probability <= 1:
            raise jev_transport.TransportError("invalid-answer")
        return {"result": "pass" if probability >= 0.5 else "fail"}, {
            "probabilities": {"pass": probability, "fail": 1 - probability},
            "threshold_id": "noul-0.5", "noul": probability, "confidence": None,
        }
    choice = item.get("choice")
    probabilities = item.get("probabilities")
    if item.get("type") != "choice" or choice not in {"ACCEPT", "PARTIAL", "DECLINE"} or not isinstance(probabilities, dict):
        raise jev_transport.TransportError("invalid-answer")
    return {"result": choice}, {
        "probabilities": probabilities, "confidence": item.get("confidence"),
    }


def _result(status: str, decision_id: str, *, sent: bool = False,
            answer: Any = None, model: str | None = None,
            latency_ms: int | None = None, usage: Any = None,
            error: str | None = None, redaction: dict | None = None) -> dict:
    return {"status": status, "decision_id": decision_id, "sent": sent,
            "answer": answer, "model": model, "latency_ms": latency_ms,
            "usage": usage, "error": error, "redaction": redaction}


def _emit(value: dict, exit_code: int) -> int:
    print(ledger.canonical_json(value))
    return exit_code


def _decision(args: argparse.Namespace, ledger_path: Path, *, ask: bool) -> int:
    decision_id = str(uuid.uuid4())
    subject = _read_json(args.subject_ref)
    if not isinstance(subject, dict):
        raise InputError("subject_ref must be an object")
    _blind(subject)
    surface = args.surface
    observations = _read_json(args.observations) if args.observations else None
    if observations is not None:
        _blind(observations)
    question = _read_json(args.question) if ask else None
    if ask:
        _blind(question)
    raw_answer = None if ask else _read_json(args.answer)
    if not ask and (not isinstance(raw_answer, dict) or set(raw_answer) != {"result"}):
        raise InputError("answer must be a one-key result object")
    question_version = ledger.sha256_of(question) if question else None
    status, exit_code = "recorded", 0
    answer, model, latency_ms, usage = raw_answer, getattr(args, "model", None), None, None
    sent, project, redact_mode, safe_state, diagnostics = False, None, None, None, {}
    redaction_report = None
    if ask:
        status, exit_code = "egress-denied", 3
        policy = _policy(args.policy)
        surface_entry = _policy_entry(policy, "surfaces", surface)
        if surface_entry:
            project = _project_identity()
            project_entry = _policy_entry(policy, "projects", project)
            if project_entry:
                question = _question(surface, question)
                redact_mode = surface_entry["redact"]
                try:
                    dictionary = redaction.load_dictionary(
                        args.redact_dictionary, os.environ.get("JUDGMENT_REDACT_PROFILE_ROOT")
                    )
                except (ValueError, OSError, UnicodeError, json.JSONDecodeError):
                    dictionary = None
                payload = {"state": observations, "model": "jev-latest",
                           "questions": {args.question_id: question}}
                clean, report = redaction.redact(payload, redact_mode, dictionary)
                if clean is None or report["blocked"]:
                    status, exit_code = "redaction-blocked", 3
                    redaction_report = report
                else:
                    request_bytes = _json_bytes(clean)
                    safe_state = clean["state"]
                    if len(request_bytes) > MAX_REQUEST_BYTES:
                        status, exit_code = "size-blocked", 3
                        safe_state = None
                    elif args.dry_run:
                        status, exit_code = "dry-run", 0
                    elif not os.environ.get("TYPESAFE_API_KEY"):
                        status, exit_code = "no-key", 0
                    else:
                        sent = True
                        start = time.monotonic()
                        try:
                            response = jev_transport.send(request_bytes, os.environ["TYPESAFE_API_KEY"])
                            latency_ms = round((time.monotonic() - start) * 1000)
                            answer, diagnostics = _answer(surface, response, args.question_id)
                            model = response["model"]
                            usage = response.get("usage")
                            status, exit_code = "recorded", 0
                        except (jev_transport.TransportError, ValueError, TypeError,
                                OverflowError, KeyError):
                            latency_ms = round((time.monotonic() - start) * 1000)
                            status, exit_code = "transport-error", 4
    row = ledger.record_decision(
        surface, subject, "jev" if ask else "incumbent-llm", answer,
        state=safe_state if ask else observations,
        question_id=args.question_id, question_version=question_version,
        model=model, mode="shadow", routed="none", latency_ms=latency_ms,
        error=status if status != "recorded" else None,
        decision_id=decision_id, ledger=ledger_path,
        redact_mode=redact_mode, sent=sent, usage=usage, project=project,
        redaction=redaction_report,
        source_decision_id=getattr(args, "source_decision_id", None),
        **diagnostics,
    )
    if row is None:
        return _emit(_result("ledger-error", decision_id, sent=sent, error="ledger-error"), 5)
    return _emit(_result(status, decision_id, sent=sent, answer=answer,
                         model=model, latency_ms=latency_ms, usage=usage,
                         error=status if status != "recorded" else None,
                         redaction=redaction_report), exit_code)


def _outcome(args: argparse.Namespace, ledger_path: Path) -> int:
    label = _read_json(args.label)
    if not isinstance(label, dict) or set(label) != {"result"}:
        raise InputError("label must be a one-key result object")
    row = ledger.record_outcome(args.decision_id, label, args.source,
                                note=args.note, ledger=ledger_path)
    if row is None:
        return _emit(_result("ledger-error", args.decision_id, error="ledger-error"), 5)
    return _emit(_result("recorded", args.decision_id), 0)


def _parser() -> argparse.ArgumentParser:
    parser = JudgeParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ledger", type=Path, default=Path(os.environ.get("JUDGMENT_LEDGER") or
                        (Path(os.environ.get("APPDATA") or Path.home() / "AppData" / "Roaming") /
                         "hive-manager" / "judgments" / "ledger.jsonl")))
    parser.add_argument("--tables", type=Path, default=Path(os.environ.get("JUDGMENT_TABLES") or HERE / "decision-tables.json"))
    parser.add_argument("--policy", type=Path, default=Path(os.environ.get("JUDGMENT_EGRESS_POLICY") or HERE / "egress-policy.json"))
    parser.add_argument("--redact-dictionary", default=os.environ.get("JUDGMENT_REDACT_DICTIONARY"))
    commands = parser.add_subparsers(dest="command", required=True)
    ask = commands.add_parser("ask", help="gate and send a blind shadow question")
    ask.add_argument("--surface", required=True)
    ask.add_argument("--subject-ref", required=True)
    ask.add_argument("--observations", required=True)
    ask.add_argument("--question-id", required=True)
    ask.add_argument("--question", required=True)
    ask.add_argument("--source-decision-id")
    ask.add_argument("--dry-run", action="store_true")
    record = commands.add_parser("record", help="record an incumbent locally")
    record.add_argument("--surface", required=True)
    record.add_argument("--subject-ref", required=True)
    record.add_argument("--observations")
    record.add_argument("--answer", required=True)
    record.add_argument("--question-id", required=True)
    record.add_argument("--model")
    outcome = commands.add_parser("outcome", help="append a local outcome")
    outcome.add_argument("--decision-id", required=True)
    outcome.add_argument("--label", required=True)
    outcome.add_argument("--source", required=True, choices=sorted(ledger.OUTCOME_SOURCES))
    outcome.add_argument("--note")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "ask":
            return _decision(args, args.ledger, ask=True)
        if args.command == "record":
            return _decision(args, args.ledger, ask=False)
        return _outcome(args, args.ledger)
    except InputError:
        return _emit(_result("invalid-input", "", error="invalid-input"), 2)


if __name__ == "__main__":
    raise SystemExit(main())
