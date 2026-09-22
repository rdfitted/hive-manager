#!/usr/bin/env python3
# Vendored from ~/.claude/tools/judgment/ledger.py; source SHA-256 536d9d616fb989c4854070931770122c2f9e0adedf986877863f4edd01a27549
"""judgment-ledger/v1 — the append-only record of every typed judgment we make.

WHY THIS EXISTS
---------------
A judgment that leaves no trace cannot be scored. Before this module, docket
admissions were recorded nowhere, the hive recorded one PASS/FAIL word per
milestone, and the retrieval ledger had its own shape. Three shapes meant three
auditors and no cross-surface view. This is the one shape: every judge — the
rule we run today, the LLM in the session, a typed model, a human — writes the
same two record kinds, so `audit.py` can put them side by side.

Two record kinds, one JSONL stream:

  decision  — a judge answered a question about a state.
  outcome   — ground truth arrived for a decision (a user's interview answer,
              an operator override, a human label, a downstream fact).
              Joined on decision_id. A decision may accrue many outcomes.

DESIGN CONTRACT (do not weaken):
  * FAIL-OPEN for writers. record_decision / record_outcome never raise; they
    return None on failure and log to stderr. A broken ledger must not break a
    debrief or a sync. Coverage gaps are caught by audit.py (row count vs the
    printed enumeration), not by crashing the caller.
  * APPEND-ONLY. No rewrites. Corrections are new outcome records.
  * NO ROW, NO ROUTING. A judgment must be written here before it may influence
    anything. Callers that route on a score write the row first.
  * STATE BY REFERENCE. The judged state is written to evidence/<id>.json and
    referenced by hash. It is never inlined into the ledger row.
  * NO SECRETS. Known API-key env values and key-shaped strings are scrubbed
    from both rows and evidence before they touch disk.
  * UTF-8 + LF explicitly. This is Windows; the default encoding is cp1252.
  * UNKNOWN FIELDS ARE PRESERVED. Three repos write rows; a reader must never
    drop a field it does not recognise.

Paths: ledger at ~/.claude/hooks/logs/judgments/ledger.jsonl (override with the
JUDGMENT_LEDGER env var — tests always do). evidence/ and dockets/ live beside
it. hooks/logs/ is excluded from /sync, so runtime data (which carries client
content) never reaches the agent-setup repo.

Schema: ledger-schema.json beside this file. Spec: README.md.
Tracking: rdfitted/Claude-Code-Setup#24 (epic #23).
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Iterator

SCHEMA_VERSION = 1
HERE = Path(__file__).resolve().parent
SCHEMA_PATH = HERE / "ledger-schema.json"
TABLES_PATH = HERE / "decision-tables.json"

KINDS = {"decision", "outcome"}
JUDGES = {"jev", "incumbent-llm", "code", "human"}
MODES = {"shadow", "advisory", "gating"}
# model-ack: the model that did the work ticked what was relevant (retrieval_ack.py).
# A self-report — lowest authority below the human sources, and cross-checked.
OUTCOME_SOURCES = {"user-answer", "operator-override", "human-label", "model-ack", "downstream"}

# Env vars whose *values* must never reach disk. Scrubbed by value, which is
# precise: it catches the real key wherever a caller accidentally put it.
SECRET_ENV_VARS = (
    "TYPESAFE_API_KEY",
    "AI_GATEWAY_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
)
# Shape-based fallbacks for keys that are not in this process's environment.
SECRET_PATTERNS = (
    re.compile(r"sk-[A-Za-z0-9_\-]{20,}"),
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._\-]{16,}"),
)


def _home() -> Path:
    return Path(os.environ.get("USERPROFILE") or os.environ.get("HOME") or Path.home())


def ledger_path(override: str | Path | None = None) -> Path:
    if override:
        return Path(override)
    env = os.environ.get("JUDGMENT_LEDGER")
    if env:
        return Path(env)
    return _home() / ".claude" / "hooks" / "logs" / "judgments" / "ledger.jsonl"


def evidence_dir(ledger: str | Path | None = None) -> Path:
    return ledger_path(ledger).parent / "evidence"


def dockets_dir(ledger: str | Path | None = None) -> Path:
    return ledger_path(ledger).parent / "dockets"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


# ---------------------------------------------------------------------------
# Hashing — the thing that makes repeatability measurable
# ---------------------------------------------------------------------------

def canonical_json(obj: Any) -> str:
    """Sorted keys, no whitespace, UTF-8 preserved. Same input → same bytes on
    every machine. Floats use Python's shortest-repr, which is stable."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha256_of(obj: Any) -> str:
    return "sha256:" + hashlib.sha256(canonical_json(obj).encode("utf-8")).hexdigest()


def question_version(instructions: Any, criteria: Any) -> str:
    """Hash of exactly what is sent. Changing a prompt starts a new baseline,
    and the auditor must be able to see that rather than compare across it."""
    return sha256_of({"instructions": instructions, "criteria": criteria})


# ---------------------------------------------------------------------------
# Secret scrubbing
# ---------------------------------------------------------------------------

def scrub(text: str) -> str:
    for name in SECRET_ENV_VARS:
        val = os.environ.get(name)
        if val and len(val) >= 8 and val in text:
            text = text.replace(val, f"[REDACTED:{name}]")
    for pat in SECRET_PATTERNS:
        text = pat.sub("[REDACTED]", text)
    return text


def _scrub_obj(obj: Any) -> Any:
    """Round-trip through JSON so scrubbing sees every string, then parse back."""
    return json.loads(scrub(json.dumps(obj, ensure_ascii=False)))


# ---------------------------------------------------------------------------
# Locked append — two writers must never interleave or lose a line
# ---------------------------------------------------------------------------

@contextlib.contextmanager
def _locked(path: Path):
    """Exclusive lock on a sidecar .lock file for the duration of one append.

    O_APPEND alone is not enough on Windows: the CRT seeks to EOF and then
    writes as two steps, so two processes can land on the same offset and one
    line is lost. An OS lock is released automatically if the holder dies, so
    there is no stale-lock cleanup to get wrong.
    """
    lock_path = path.with_suffix(path.suffix + ".lock")
    fh = open(lock_path, "a+b")
    try:
        if os.name == "nt":
            import msvcrt

            fh.seek(0)
            # LK_LOCK itself retries for ~10 s before raising. Allow a few
            # rounds, then give up: the writer is fail-open, so a stuck lock
            # costs one row (visible in the audit), never a hung caller.
            for attempt in range(6):
                try:
                    msvcrt.locking(fh.fileno(), msvcrt.LK_LOCK, 1)
                    break
                except OSError:
                    if attempt == 5:
                        raise
        else:
            import fcntl

            fcntl.flock(fh.fileno(), fcntl.LOCK_EX)
        yield
    finally:
        try:
            if os.name == "nt":
                import msvcrt

                fh.seek(0)
                msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(fh.fileno(), fcntl.LOCK_UN)
        finally:
            fh.close()


def _append(record: dict, ledger: str | Path | None = None) -> None:
    path = ledger_path(ledger)
    path.parent.mkdir(parents=True, exist_ok=True)
    line = scrub(json.dumps(record, ensure_ascii=False)) + "\n"
    with _locked(path):
        with open(path, "a", encoding="utf-8", newline="\n") as f:
            f.write(line)


def _warn(msg: str) -> None:
    try:
        sys.stderr.write(f"[judgment-ledger] {msg}\n")
    except Exception:
        pass


# ---------------------------------------------------------------------------
# Minimal stdlib validation (runtime). Full JSON Schema validation is in
# validate_file(), which uses jsonschema when it is installed.
# ---------------------------------------------------------------------------

_DECISION_REQUIRED = ("v", "kind", "ts", "decision_id", "surface", "subject_ref", "judge", "mode", "answer")
_OUTCOME_REQUIRED = ("v", "kind", "ts", "decision_id", "label", "source")


def validate_record(rec: dict) -> list[str]:
    errs: list[str] = []
    if not isinstance(rec, dict):
        return ["record is not an object"]
    kind = rec.get("kind")
    if kind not in KINDS:
        return [f"kind must be one of {sorted(KINDS)}, got {kind!r}"]
    required = _DECISION_REQUIRED if kind == "decision" else _OUTCOME_REQUIRED
    for field in required:
        if field not in rec:
            errs.append(f"missing required field {field!r}")
    if "v" in rec and not isinstance(rec.get("v"), int):
        errs.append("v must be an integer")
    if kind == "decision":
        if rec.get("judge") not in JUDGES:
            errs.append(f"judge must be one of {sorted(JUDGES)}")
        if rec.get("mode") not in MODES:
            errs.append(f"mode must be one of {sorted(MODES)}")
        if "subject_ref" in rec and not isinstance(rec["subject_ref"], dict):
            errs.append("subject_ref must be an object")
    else:
        if rec.get("source") not in OUTCOME_SOURCES:
            errs.append(f"source must be one of {sorted(OUTCOME_SOURCES)}")
    return errs


# ---------------------------------------------------------------------------
# Writers
# ---------------------------------------------------------------------------

def write_evidence(state: Any, decision_id: str, ledger: str | Path | None = None) -> tuple[str, str]:
    """Persist the judged state; return (state_hash, state_ref).

    The hash is over the scrubbed state, so a replay of the evidence file
    rehashes to exactly the recorded value.
    """
    clean = _scrub_obj(state)
    h = sha256_of(clean)
    d = evidence_dir(ledger)
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{decision_id}.json").write_text(canonical_json(clean), encoding="utf-8", newline="\n")
    return h, f"evidence/{decision_id}.json"


def load_evidence(state_ref: str, ledger: str | Path | None = None) -> Any:
    path = ledger_path(ledger).parent / state_ref
    return json.loads(path.read_text(encoding="utf-8"))


def record_decision(
    surface: str,
    subject_ref: dict,
    judge: str,
    answer: Any,
    *,
    state: Any = None,
    state_hash: str | None = None,
    state_ref: str | None = None,
    question_id: str | None = None,
    question_version: str | None = None,
    model: str | None = None,
    sampling: dict | None = None,
    mode: str = "shadow",
    probabilities: dict | None = None,
    confidence: float | None = None,
    threshold_id: str | None = None,
    routed: str = "none",
    latency_ms: int | None = None,
    cost_usd: float | None = None,
    error: str | None = None,
    decision_id: str | None = None,
    ledger: str | Path | None = None,
    **extra: Any,
) -> dict | None:
    """Append one decision row. Returns the row, or None on failure (fail-open).

    Pass `state` to have it written to evidence/ and hashed; or pass an existing
    `state_hash`/`state_ref` when several decisions judge the same state.
    `sampling` is recorded verbatim; None means *unknown*, which the auditor
    reports as not comparable — never assume a judge ran at temperature 0.
    """
    try:
        did = decision_id or str(uuid.uuid4())
        if state is not None and state_hash is None:
            state_hash, state_ref = write_evidence(state, did, ledger)
        rec: dict[str, Any] = {
            "v": SCHEMA_VERSION,
            "kind": "decision",
            "ts": now_iso(),
            "decision_id": did,
            "surface": surface,
            "subject_ref": subject_ref,
            "state_hash": state_hash,
            "state_ref": state_ref,
            "question_id": question_id,
            "question_version": question_version,
            "judge": judge,
            "model": model,
            "sampling": sampling,
            "mode": mode,
            "answer": answer,
            "probabilities": probabilities,
            "confidence": confidence,
            "threshold_id": threshold_id,
            "routed": routed,
            "latency_ms": latency_ms,
            "cost_usd": cost_usd,
            "error": error,
        }
        rec.update(extra)
        errs = validate_record(rec)
        if errs:
            _warn(f"decision not written ({surface}): {'; '.join(errs)}")
            return None
        _append(rec, ledger)
        return rec
    except Exception as e:  # fail-open
        _warn(f"decision not written ({surface}): {e}")
        return None


def record_outcome(
    decision_id: str,
    label: Any,
    source: str,
    *,
    note: str | None = None,
    ledger: str | Path | None = None,
    **extra: Any,
) -> dict | None:
    try:
        rec: dict[str, Any] = {
            "v": SCHEMA_VERSION,
            "kind": "outcome",
            "ts": now_iso(),
            "decision_id": decision_id,
            "label": label,
            "source": source,
        }
        if note is not None:
            rec["note"] = note
        rec.update(extra)
        errs = validate_record(rec)
        if errs:
            _warn(f"outcome not written ({decision_id}): {'; '.join(errs)}")
            return None
        _append(rec, ledger)
        return rec
    except Exception as e:
        _warn(f"outcome not written ({decision_id}): {e}")
        return None


# ---------------------------------------------------------------------------
# Readers
# ---------------------------------------------------------------------------

def read_records(paths: Iterable[str | Path] | None = None, stats: dict | None = None) -> Iterator[dict]:
    """Yield every well-formed row, unknown fields intact. Malformed lines are
    counted in stats['malformed'] rather than silently vanishing."""
    if paths is None:
        base = ledger_path()
        paths = sorted(base.parent.glob("ledger*.jsonl")) if base.parent.exists() else []
    for p in paths:
        p = Path(p)
        if not p.exists():
            continue
        with open(p, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    if stats is not None:
                        stats["malformed"] = stats.get("malformed", 0) + 1
                    continue
                if isinstance(rec, dict):
                    yield rec


def validate_file(paths: Iterable[str | Path] | None = None) -> tuple[int, list[str]]:
    """Validate every row against ledger-schema.json. Uses jsonschema when
    available, else the stdlib checks. Returns (rows_checked, errors)."""
    errors: list[str] = []
    n = 0
    validator = None
    try:
        import jsonschema  # type: ignore

        schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
        validator = jsonschema.Draft202012Validator(schema)
    except Exception:
        validator = None
    stats: dict = {}
    for rec in read_records(paths, stats):
        n += 1
        if validator is not None:
            for err in validator.iter_errors(rec):
                errors.append(f"row {n} ({rec.get('decision_id', '?')}): {err.message}")
        else:
            for err in validate_record(rec):
                errors.append(f"row {n} ({rec.get('decision_id', '?')}): {err}")
    if stats.get("malformed"):
        errors.append(f"{stats['malformed']} malformed line(s)")
    return n, errors


def load_tables(path: str | Path | None = None) -> dict:
    p = Path(path) if path else TABLES_PATH
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except Exception as e:
        _warn(f"decision tables unreadable ({p}): {e}")
        return {}


# ---------------------------------------------------------------------------
# CLI — so markdown-driven skills can write rows without Python glue
# ---------------------------------------------------------------------------

def _json_arg(s: str | None) -> Any:
    if s is None:
        return None
    try:
        return json.loads(s)
    except json.JSONDecodeError:
        return s  # a bare string answer is fine


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="judgment-ledger/v1 writer and validator")
    ap.add_argument("--ledger", help="ledger file (default: $JUDGMENT_LEDGER or ~/.claude/hooks/logs/judgments/ledger.jsonl)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    d = sub.add_parser("record-decision", help="append a decision row; prints its decision_id")
    d.add_argument("--surface", required=True)
    d.add_argument("--subject", required=True, help="JSON object identifying what was judged")
    d.add_argument("--judge", required=True, choices=sorted(JUDGES))
    d.add_argument("--answer", required=True, help="JSON value or bare string")
    d.add_argument("--state-file", help="JSON file holding the judged state (written to evidence/)")
    d.add_argument("--mode", default="shadow", choices=sorted(MODES))
    d.add_argument("--model")
    d.add_argument("--question-id")
    d.add_argument("--routed", default="none")

    o = sub.add_parser("record-outcome", help="append an outcome row")
    o.add_argument("--decision-id", required=True)
    o.add_argument("--label", required=True)
    o.add_argument("--source", required=True, choices=sorted(OUTCOME_SOURCES))
    o.add_argument("--note")

    sub.add_parser("validate", help="validate every row against ledger-schema.json")
    h = sub.add_parser("hash", help="print the canonical state hash of a JSON file")
    h.add_argument("file")
    sub.add_parser("path", help="print the resolved ledger path")

    args = ap.parse_args(argv)

    if args.cmd == "record-decision":
        state = None
        if args.state_file:
            state = json.loads(Path(args.state_file).read_text(encoding="utf-8"))
        rec = record_decision(
            args.surface,
            _json_arg(args.subject),
            args.judge,
            _json_arg(args.answer),
            state=state,
            mode=args.mode,
            model=args.model,
            question_id=args.question_id,
            routed=args.routed,
            ledger=args.ledger,
        )
        if rec:
            print(rec["decision_id"])
        return 0  # fail-open: a warning was printed if the row was not written

    if args.cmd == "record-outcome":
        rec = record_outcome(args.decision_id, _json_arg(args.label), args.source, note=args.note, ledger=args.ledger)
        if rec:
            print("ok")
        return 0

    if args.cmd == "validate":
        paths = [args.ledger] if args.ledger else None
        n, errors = validate_file(paths)
        print(f"{n} row(s) checked, {len(errors)} error(s)")
        for e in errors[:50]:
            print(f"  {e}")
        return 1 if errors else 0

    if args.cmd == "hash":
        print(sha256_of(_scrub_obj(json.loads(Path(args.file).read_text(encoding="utf-8")))))
        return 0

    if args.cmd == "path":
        print(ledger_path(args.ledger))
        return 0

    return 2


if __name__ == "__main__":
    sys.exit(main())
