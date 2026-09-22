#!/usr/bin/env python3
"""Offline, read-only replay of planning retrieval over saved Hive sessions."""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import re
import subprocess
import tempfile
import uuid
from collections import Counter
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Iterator, Optional

import ledger as judgment_ledger
from plan_grammar import parse_plan_markdown_with_diagnostics
from retrieval_rules import (
    STALE_SCOPE_OMISSION_DETAILS,
    TASK_PATH_UNRESOLVED_DETAIL,
    run_ruleset,
)


RULESET_ORDER = ("current", "hv10", "hv11", "hv10+hv11", "hv12")
COLUMN_ENTRY_MODES = {
    "current": "compose",
    "hv10": "plan-ready",
    "hv11": "plan-ready",
    "hv10+hv11": "plan-ready",
    "hv12": "plan-ready",
    "production_attached": "observed",
}
ENTRY_COMPARISON_METRICS = (
    "tasks_with_touches",
    "tasks_with_knowledge",
    "knowledge_pairs",
)
DECISION_NAMESPACE = uuid.NAMESPACE_URL
GRAMMAR_TASK = re.compile(
    r"^\s*[-*]\s+\[[ xX]\]\s+(?:\[[^\]]+\]\s+)*T\d+:", re.MULTILINE
)
KNOWLEDGE_FILES = (
    "project-dna.md",
    "bug-patterns.md",
    "learnings.jsonl",
    "curation-state.json",
)
NOTE_REASONS = (
    "no-scope",
    "scope-unresolved",
    "hub-withheld",
    "no-task-touches",
    "codegraph-unavailable",
)
TASK_STATUSES = ("declared", "contract-path", "undeclared", "partial", "unresolved")
SPAWN_CONTEXT_SCHEMA_VERSION = "hive.spawn-context/v1"
KNOWLEDGE_ACK_SCHEMA_VERSION = "hive.knowledge-ack/v1"
PROXY_STOP_WORDS = {
    "because",
    "between",
    "through",
    "without",
    "another",
    "against",
    "anything",
    "everything",
    "something",
    "whether",
    "however",
    "instead",
    "already",
    "current",
    "project",
    "session",
}
SPOT_CHECK_FIELDS = (
    "repo",
    "session_id",
    "agent_id",
    "cli",
    "plan_task_id",
    "ack_state",
    "ack_tags",
    "tagged_references",
    "used_references",
    "proxy_agreement",
    "human_used_tags",
    "human_agreement",
    "review_notes",
)


def evaluate_fixture(
    root: Path, rulesets: Iterable[str] = RULESET_ORDER
) -> dict[str, dict]:
    """Evaluate one materialized project fixture in stable rule-set order."""

    return {ruleset: run_ruleset(ruleset, root) for ruleset in rulesets}


def evaluate_replay_session(
    repo: Path,
    session: Path,
    inventory: list[str],
) -> tuple[dict[str, dict], dict[str, tuple[dict, dict]]]:
    """Evaluate replay columns at their explicit production entry modes."""

    with materialized_session(
        repo, session, inventory, entry=COLUMN_ENTRY_MODES["current"]
    ) as compose_root:
        compose_matrix = evaluate_fixture(compose_root)
    with materialized_session(
        repo, session, inventory, entry=COLUMN_ENTRY_MODES["hv10"]
    ) as plan_ready_root:
        plan_ready_matrix = evaluate_fixture(plan_ready_root, RULESET_ORDER[1:])

    matrix = {"current": compose_matrix["current"]}
    comparisons = {}
    for ruleset in RULESET_ORDER[1:]:
        matrix[ruleset] = plan_ready_matrix[ruleset]
        comparisons[ruleset] = (
            compose_matrix[ruleset],
            plan_ready_matrix[ruleset],
        )
    return matrix, comparisons


def resolve_ledger_path(override: Optional[Path] = None) -> Path:
    if override is not None:
        return override
    environment = os.environ.get("JUDGMENT_LEDGER")
    if environment:
        return Path(environment)
    appdata = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
    return appdata / "hive-manager" / "judgments" / "ledger.jsonl"


def resolve_session_store_root(override: Optional[Path] = None) -> Path:
    if override is not None:
        return override
    appdata = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
    return appdata / "hive-manager" / "sessions"


def observed_production_attachments(
    session_store_root: Path, session_id: str
) -> tuple[int, Optional[str]]:
    graph_path = session_store_root / session_id / "state" / "work-graph.json"
    if not graph_path.is_file():
        return 0, "production work graph missing"
    try:
        graph = json.loads(graph_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return 0, f"production work graph unreadable: {error}"
    edges = graph.get("edges") if isinstance(graph, dict) else None
    if not isinstance(edges, list):
        return 0, "production work graph has no edge list"
    return (
        sum(
            1
            for edge in edges
            if isinstance(edge, dict)
            and edge.get("kind") == "informs"
            and edge.get("provenance") == "knowledge"
        ),
        None,
    )


def load_spawn_contexts(session: Path) -> tuple[list[dict], list[str]]:
    contexts = []
    errors = []
    prompts = session / "prompts"
    if not prompts.is_dir():
        return contexts, errors
    for path in sorted(prompts.glob("*-context.json")):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            errors.append(f"{path.name}: {error}")
            continue
        if not isinstance(value, dict):
            errors.append(f"{path.name}: sidecar is not an object")
            continue
        if value.get("schema_version") != SPAWN_CONTEXT_SCHEMA_VERSION:
            errors.append(f"{path.name}: unsupported spawn-context schema")
            continue
        if not isinstance(value.get("agent_id"), str):
            errors.append(f"{path.name}: missing agent_id")
            continue
        if not isinstance(value.get("kept"), list) or not isinstance(
            value.get("dropped"), list
        ):
            errors.append(f"{path.name}: kept and dropped must be arrays")
            continue
        contexts.append(value)
    return contexts, errors


def load_latest_knowledge_acks(
    session_store_root: Path, session_id: str
) -> tuple[dict[str, set[str]], list[str]]:
    path = session_store_root / session_id / "state" / "knowledge-acks.jsonl"
    if not path.is_file():
        return {}, []
    acknowledgements = {}
    errors = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        return {}, [f"knowledge acknowledgement store unreadable: {error}"]
    for line_number, line in enumerate(lines, start=1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            errors.append(f"knowledge acknowledgement line {line_number}: {error}")
            continue
        tags = value.get("knowledge_ack") if isinstance(value, dict) else None
        if (
            not isinstance(value, dict)
            or value.get("schema_version") != KNOWLEDGE_ACK_SCHEMA_VERSION
            or value.get("session_id") != session_id
            or not isinstance(value.get("agent_id"), str)
            or not isinstance(tags, list)
            or any(not isinstance(tag, str) for tag in tags)
        ):
            errors.append(
                f"knowledge acknowledgement line {line_number}: invalid record"
            )
            continue
        acknowledgements[value["agent_id"]] = set(tags)
    return acknowledgements, errors


def load_agent_clis(session_store_root: Path, session_id: str) -> dict[str, str]:
    path = session_store_root / session_id / "session.json"
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    agents = value.get("agents", []) if isinstance(value, dict) else []
    return {
        agent["id"]: agent["config"]["cli"]
        for agent in agents
        if isinstance(agent, dict)
        and isinstance(agent.get("id"), str)
        and isinstance(agent.get("config"), dict)
        and isinstance(agent["config"].get("cli"), str)
    }


def _tagged_prompt_excerpts(session: Path, agent_id: str) -> dict[str, str]:
    path = session / "prompts" / f"{agent_id}-prompt.md"
    try:
        content = path.read_text(encoding="utf-8")
    except OSError:
        return {}
    excerpts = {}
    for line in content.splitlines():
        match = re.search(r"\[(k[1-9][0-9]*)\]\s*(.+)$", line)
        if match:
            excerpts[match.group(1)] = match.group(2).strip()
    return excerpts


def _completion_proxy_corpus(
    session: Path,
    session_id: str,
    agent_id: str,
    changed: Optional[set[str]],
) -> str:
    parts = []
    suffix = agent_id.removeprefix(f"{session_id}-")
    for path in (
        session / "tasks" / f"{suffix}-task.md",
        session / "conversations" / f"{agent_id}.md",
    ):
        try:
            parts.append(path.read_text(encoding="utf-8"))
        except OSError:
            pass
    if changed:
        parts.extend(sorted(changed))
    return "\n".join(parts)


def proxy_mentioned(excerpt: str, pointer: str, completion_corpus: str) -> bool:
    corpus = completion_corpus.lower()
    stem = Path(pointer).stem.lower() if pointer else ""
    if len(stem) >= 3 and re.search(
        r"(?<![\w-])" + re.escape(stem) + r"(?![\w-])", corpus
    ):
        return True
    words = {
        word
        for word in re.findall(r"[a-z][a-z-]{6,}", excerpt.lower())
        if word not in PROXY_STOP_WORDS
    }
    return sum(1 for word in words if word in corpus) >= 2


def wilson_interval(
    successes: int, total: int, z: float = 1.96
) -> tuple[Optional[float], Optional[float]]:
    if total <= 0:
        return None, None
    proportion = successes / total
    denominator = 1 + z * z / total
    center = (proportion + z * z / (2 * total)) / denominator
    margin = (
        z
        * math.sqrt(
            proportion * (1 - proportion) / total
            + z * z / (4 * total * total)
        )
        / denominator
    )
    return center - margin, center + margin


def _rate_summary(observations: list[dict]) -> dict:
    total = len(observations)
    used = sum(1 for observation in observations if observation["used"])
    low, high = wilson_interval(used, total)
    return {
        "used": used,
        "shown": total,
        "rate": used / total if total else None,
        "wilson_low": low,
        "wilson_high": high,
    }


def _grouped_precision(observations: list[dict], field: str) -> dict:
    groups = {}
    for observation in observations:
        key = str(observation.get(field))
        groups.setdefault(key, []).append(observation)
    return {key: _rate_summary(groups[key]) for key in sorted(groups)}


def summarize_knowledge_acks(
    sampled_spawns: list[dict], observations: list[dict]
) -> dict:
    by_cli = {}
    for spawn in sampled_spawns:
        cli = spawn["cli"]
        counts = by_cli.setdefault(cli, {"sampled_completions": 0, "acknowledged": 0})
        counts["sampled_completions"] += 1
        counts["acknowledged"] += int(spawn["acknowledged"])
    for counts in by_cli.values():
        total = counts["sampled_completions"]
        share = counts["acknowledged"] / total if total else None
        counts["compliance"] = share
        counts["reliable"] = share is not None and share >= 0.8

    kept = [observation for observation in observations if not observation["miss_sample"]]
    misses = [observation for observation in observations if observation["miss_sample"]]
    proxy_total = len(observations)
    proxy_agree = sum(
        1
        for observation in observations
        if observation["used"] == observation["proxy_mentioned"]
    )
    miss_summary = _rate_summary(misses)
    miss_summary["enough_samples"] = miss_summary["shown"] >= 100
    miss_summary["flagged_low_n"] = miss_summary["shown"] < 100
    return {
        "compliance_by_cli": dict(sorted(by_cli.items())),
        "precision": {
            "overall": _rate_summary(kept),
            "by_cli": _grouped_precision(kept, "cli"),
            "by_position": _grouped_precision(kept, "position"),
            "by_origin": _grouped_precision(kept, "origin"),
            "by_provenance": _grouped_precision(kept, "provenance"),
        },
        "miss_rate": miss_summary,
        "proxy_agreement": {
            "agree": proxy_agree,
            "total": proxy_total,
            "rate": proxy_agree / proxy_total if proxy_total else None,
        },
    }


def _spawn_reference_rows(context: dict) -> Iterator[tuple[int, dict, dict]]:
    miss_tags = set(context.get("miss_sample_references", []))
    reference_index = 0
    for reference in context["kept"]:
        if not isinstance(reference, dict):
            continue
        tag = reference.get("tag")
        yield reference_index, reference, {
            "disposition": "kept",
            "reason": None,
            "position": reference.get("position"),
            "origin": reference.get("origin"),
            "provenance": reference.get("source"),
            "priority": reference.get("priority"),
            "cost": reference.get("chars"),
            "miss_sample": tag in miss_tags,
        }
        reference_index += 1
    for reference in context["dropped"]:
        if not isinstance(reference, dict):
            continue
        yield reference_index, reference, {
            "disposition": "dropped",
            "reason": reference.get("reason"),
            "position": None,
            "origin": reference.get("origin"),
            "provenance": reference.get("source"),
            "priority": reference.get("priority"),
            "cost": reference.get("chars"),
            "miss_sample": False,
        }
        reference_index += 1


def _spawn_decision_id(
    repo_name: str, session_id: str, agent_id: str, reference_index: int
) -> str:
    return _decision_id(
        repo_name,
        session_id,
        "hive.retrieval.spawn",
        agent_id,
        str(reference_index),
    )


def _record_spawn_context(
    *,
    ledger_path: Path,
    existing_decisions: set[str],
    repo_name: str,
    session_id: str,
    context: dict,
) -> None:
    agent_id = context["agent_id"]
    for reference_index, reference, answer in _spawn_reference_rows(context):
        decision_id = _spawn_decision_id(
            repo_name, session_id, agent_id, reference_index
        )
        if decision_id in existing_decisions:
            continue
        subject = {
            "repo": repo_name,
            "session_id": session_id,
            "agent_id": agent_id,
            "plan_task_id": context.get("plan_task_id"),
            "reference_index": reference_index,
            "tag": reference.get("tag"),
            "pointer": reference.get("pointer"),
        }
        judgment_ledger.record_decision(
            "hive.retrieval.spawn",
            subject,
            "code",
            answer,
            question_id="knowledge_ack",
            question_version=str(context.get("question_version", "")) or None,
            mode="shadow",
            decision_id=decision_id,
            ledger=ledger_path,
        )
        existing_decisions.add(decision_id)


def _evaluate_spawn_ack(
    *,
    ledger_path: Path,
    existing_outcomes: set[tuple[str, str, str]],
    repo_name: str,
    session: Path,
    context: dict,
    acknowledgement: Optional[set[str]],
    cli: str,
    changed: Optional[set[str]],
) -> tuple[Optional[dict], list[dict], Optional[dict]]:
    if not context.get("sampled"):
        return None, [], None
    agent_id = context["agent_id"]
    session_id = session.name
    sampled_spawn = {
        "cli": cli,
        "acknowledged": acknowledgement is not None,
    }
    excerpts = _tagged_prompt_excerpts(session, agent_id)
    corpus = _completion_proxy_corpus(
        session, session_id, agent_id, changed
    )
    observations = []
    agreement_count = 0
    tagged_references = 0
    used_references = 0
    for reference_index, reference, answer in _spawn_reference_rows(context):
        tag = reference.get("tag")
        if answer["disposition"] != "kept" or not isinstance(tag, str):
            continue
        tagged_references += 1
        if acknowledgement is None:
            continue
        used = tag in acknowledgement
        used_references += int(used)
        mentioned = proxy_mentioned(
            excerpts.get(tag, ""), str(reference.get("pointer") or ""), corpus
        )
        agreement_count += int(used == mentioned)
        decision_id = _spawn_decision_id(
            repo_name, session_id, agent_id, reference_index
        )
        label = "used" if used else "unused"
        outcome_key = (decision_id, label, "model-ack")
        if outcome_key not in existing_outcomes:
            judgment_ledger.record_outcome(
                decision_id,
                label,
                "model-ack",
                note="worker completion knowledge acknowledgement",
                ledger=ledger_path,
                session_id=session_id,
                agent_id=agent_id,
                tag=tag,
                proxy_mentioned=mentioned,
            )
            existing_outcomes.add(outcome_key)
        observations.append(
            {
                "used": used,
                "position": answer["position"],
                "origin": answer["origin"],
                "provenance": answer["provenance"],
                "miss_sample": answer["miss_sample"],
                "proxy_mentioned": mentioned,
                "cli": cli,
            }
        )
    spot_check = None if acknowledgement is None else {
        "repo": repo_name,
        "session_id": session_id,
        "agent_id": agent_id,
        "cli": cli,
        "plan_task_id": context.get("plan_task_id") or "",
        "ack_state": "decided",
        "ack_tags": " ".join(sorted(acknowledgement or [])),
        "tagged_references": tagged_references,
        "used_references": used_references,
        "proxy_agreement": (
            agreement_count / tagged_references
            if tagged_references
            else ""
        ),
        "human_used_tags": "",
        "human_agreement": "",
        "review_notes": "",
    }
    return sampled_spawn, observations, spot_check


def _evaluate_session_acks(
    *,
    ledger_path: Path,
    existing_outcomes: set[tuple[str, str, str]],
    repo_name: str,
    session: Path,
    session_store_root: Path,
    contexts: list[dict],
    changed: Optional[set[str]],
) -> tuple[list[dict], list[dict], list[dict], list[str]]:
    acknowledgements, errors = load_latest_knowledge_acks(
        session_store_root, session.name
    )
    agent_clis = load_agent_clis(session_store_root, session.name)
    sampled_spawns = []
    observations = []
    spot_checks = []
    for context in contexts:
        agent_id = context["agent_id"]
        sampled_spawn, context_observations, spot_check = _evaluate_spawn_ack(
            ledger_path=ledger_path,
            existing_outcomes=existing_outcomes,
            repo_name=repo_name,
            session=session,
            context=context,
            acknowledgement=acknowledgements.get(agent_id),
            cli=agent_clis.get(agent_id, "unknown"),
            changed=changed,
        )
        if sampled_spawn is not None:
            sampled_spawns.append(sampled_spawn)
        observations.extend(context_observations)
        if spot_check is not None:
            spot_checks.append(spot_check)
    return sampled_spawns, observations, spot_checks, errors


def write_spot_check_csv(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=SPOT_CHECK_FIELDS)
        writer.writeheader()
        writer.writerows(
            sorted(
                rows,
                key=lambda row: (
                    row["repo"],
                    row["session_id"],
                    row["agent_id"],
                ),
            )[:30]
        )


def _delivery_coverage(worker_spawns: int, planned_spawns: int) -> dict:
    return {
        "worker_spawns": worker_spawns,
        "spawns_with_plan_task_id": planned_spawns,
        "share": planned_spawns / worker_spawns if worker_spawns else 0.0,
    }


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
        return True
    except ValueError:
        return False


def _session_cutoff(plan_path: Path) -> datetime:
    return datetime.fromtimestamp(plan_path.stat().st_mtime, timezone.utc)


def _parse_timestamp(value: object) -> Optional[datetime]:
    if not isinstance(value, str) or not value.strip():
        return None
    candidate = value.strip()
    if len(candidate) == 10:
        candidate += "T23:59:59+00:00"
    candidate = candidate.replace("Z", "+00:00")
    try:
        parsed = datetime.fromisoformat(candidate)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def _filter_learnings(content: str, cutoff: datetime) -> str:
    kept = []
    for line in content.splitlines():
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        timestamp = _parse_timestamp(value.get("ts") or value.get("date"))
        if timestamp is not None and timestamp <= cutoff:
            kept.append(json.dumps(value, ensure_ascii=False, separators=(",", ":")))
    return "".join(f"{line}\n" for line in kept)


def _git(repo: Path, arguments: list[str]) -> subprocess.CompletedProcess[bytes]:
    if any(argument in {"fetch", "pull", "push"} for argument in arguments):
        raise ValueError("replay git plumbing is local and read-only")
    return subprocess.run(
        ["git", "-C", str(repo), *arguments],
        capture_output=True,
        check=False,
    )


def _local_ref(repo: Path, refs: Iterable[str]) -> Optional[str]:
    for reference in refs:
        result = _git(repo, ["show-ref", "--verify", "--quiet", reference])
        if result.returncode == 0:
            return reference
    return None


def _resolved_commit(repo: Path, reference: str) -> Optional[str]:
    result = _git(repo, ["rev-parse", "--verify", f"{reference}^{{commit}}"])
    if result.returncode != 0:
        return None
    commit = result.stdout.decode("utf-8", errors="replace").strip()
    return commit or None


def _diff_paths(repo: Path, base: str, tip: str) -> Optional[set[str]]:
    changed = _git(repo, ["diff", "--name-only", "-z", base, tip, "--"])
    if changed.returncode != 0:
        return None
    return {
        value.replace("\\", "/").lower()
        for value in changed.stdout.decode("utf-8", errors="replace").split("\0")
        if value
    }


def _recover_merged_diff(
    repo: Path, main: str, branch_tip: str
) -> Optional[set[str]]:
    merges = _git(
        repo,
        ["rev-list", "--merges", "--ancestry-path", f"{branch_tip}..{main}"],
    )
    if merges.returncode != 0:
        return None
    for merge in merges.stdout.decode("utf-8", errors="replace").splitlines():
        parents = _git(repo, ["rev-list", "--parents", "-n", "1", merge])
        if parents.returncode != 0:
            continue
        commits = parents.stdout.decode("utf-8", errors="replace").split()
        if len(commits) < 3 or commits[2] != branch_tip:
            continue
        base = _git(repo, ["merge-base", commits[1], branch_tip])
        if base.returncode != 0:
            continue
        first_parent_base = base.stdout.decode("utf-8", errors="replace").strip()
        if not first_parent_base:
            continue
        changed = _diff_paths(repo, first_parent_base, branch_tip)
        if changed:
            return changed
    return None


def recover_changed_files(repo: Path, session_id: str) -> tuple[Optional[set[str]], str]:
    branch = _local_ref(
        repo,
        (
            f"refs/heads/hive/{session_id}/primary",
            f"refs/remotes/origin/hive/{session_id}/primary",
        ),
    )
    if branch is None:
        return None, "no recoverable diff: local session ref missing"
    main = _local_ref(repo, ("refs/heads/main", "refs/remotes/origin/main"))
    if main is None:
        return None, "no recoverable diff: local main ref missing"
    branch_tip = _resolved_commit(repo, branch)
    if branch_tip is None:
        return None, "no recoverable diff: local session tip unavailable"
    merge_base = _git(repo, ["merge-base", main, branch_tip])
    if merge_base.returncode != 0:
        return None, "no recoverable diff: merge-base unavailable"
    base = merge_base.stdout.decode("utf-8", errors="replace").strip()
    if not base:
        return None, "no recoverable diff: merge-base unavailable"
    changed = _diff_paths(repo, base, branch_tip)
    if changed is None:
        return None, "no recoverable diff: local diff failed"
    if changed:
        return changed, "recovered from local ref"
    if base == branch_tip:
        merged = _recover_merged_diff(repo, main, branch_tip)
        if merged:
            return merged, "recovered from local merged branch"
        return None, "no recoverable diff: local merge history was unavailable"
    return None, "no recoverable diff: local branch diff was empty"


def tracked_files(repo: Path) -> tuple[list[str], Optional[str]]:
    result = _git(repo, ["ls-files", "-z"])
    if result.returncode != 0:
        return [], "tracked file inventory unavailable"
    paths = sorted(
        {
            value.replace("\\", "/")
            for value in result.stdout.decode("utf-8", errors="replace").split("\0")
            if value and not value.startswith(".ai-docs/")
        }
    )
    return paths, None


def _plan_classification(content: str) -> tuple[str, Optional[str]]:
    if GRAMMAR_TASK.search(content) is None:
        return "pre-grammar", "no planning retrieval possible"
    plan, diagnostics = parse_plan_markdown_with_diagnostics(content)
    if diagnostics:
        return "unparseable", diagnostics[0]
    explicit = [task for task in plan.tasks if task.explicit_id]
    if not explicit:
        return "unparseable", "task grammar marker produced no explicit tasks"
    return "parseable", None


def discover_sessions(sessions_root: Path) -> Iterator[Path]:
    if not sessions_root.is_dir():
        return
    for child in sorted(sessions_root.iterdir(), key=lambda path: path.name):
        if child.is_dir() and (child / "plan.md").is_file():
            yield child


@contextmanager
def materialized_session(
    repo: Path,
    session: Path,
    inventory: list[str],
    *,
    entry: str,
) -> Iterator[Path]:
    if entry not in {"compose", "plan-ready"}:
        raise ValueError("replay entry must be compose or plan-ready")
    with tempfile.TemporaryDirectory(prefix="hive-retrieval-") as temporary:
        root = Path(temporary) / "project"
        ai_docs = root / ".ai-docs"
        ai_docs.mkdir(parents=True)
        plan_path = session / "plan.md"
        (root / "plan.md").write_text(
            plan_path.read_text(encoding="utf-8").replace("\r\n", "\n").replace("\r", "\n"),
            encoding="utf-8",
            newline="\n",
        )
        cutoff = _session_cutoff(plan_path)
        source_docs = repo / ".ai-docs"
        defaults = {
            "project-dna.md": "# Project DNA\n",
            "bug-patterns.md": "# Bug Patterns\n",
            "learnings.jsonl": "",
            "curation-state.json": '{"last_curated_line":0}\n',
        }
        for filename in KNOWLEDGE_FILES:
            source = source_docs / filename
            content = source.read_text(encoding="utf-8") if source.is_file() else defaults[filename]
            content = content.replace("\r\n", "\n").replace("\r", "\n")
            if filename == "learnings.jsonl":
                content = _filter_learnings(content, cutoff)
                curated = sum(1 for line in content.splitlines() if line.strip())
                (ai_docs / "curation-state.json").write_text(
                    json.dumps({"last_curated_line": curated}) + "\n",
                    encoding="utf-8",
                    newline="\n",
                )
            if filename == "curation-state.json" and (ai_docs / filename).exists():
                continue
            (ai_docs / filename).write_text(content, encoding="utf-8", newline="\n")
        (root / "files.txt").write_text(
            "".join(f"{path}\n" for path in inventory), encoding="utf-8", newline="\n"
        )
        (root / "entry.txt").write_text(
            f"{entry}\n", encoding="utf-8", newline="\n"
        )
        artifact_path = session / "artifacts" / "codegraph.json"
        if artifact_path.is_file():
            try:
                artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
                artifact["root"] = str(root.resolve())
                (root / "codegraph.json").write_text(
                    json.dumps(artifact, indent=2) + "\n", encoding="utf-8", newline="\n"
                )
            except (json.JSONDecodeError, OSError):
                pass
        yield root


def _overlaps(paths: Iterable[str], changed: set[str]) -> bool:
    for raw in paths:
        path = raw.replace("\\", "/").lower().strip("/")
        if path == "*":
            return bool(changed)
        for changed_path in changed:
            if (
                path == changed_path
                or changed_path.startswith(f"{path}/")
                or path.startswith(f"{changed_path}/")
            ):
                return True
    return False


def _path_relevance(context_node: dict, changed: Optional[set[str]]) -> Optional[bool]:
    scope = context_node.get("scope", [])
    if not scope or changed is None:
        return None
    return _overlaps(scope, changed)


def _task_status(result: dict, task_id: str) -> str:
    declared = result["declared_touches"].get(task_id, [])
    knowledge = result["knowledge_attachment_touches"].get(task_id, [])
    resolution_failed = bool(
        result.get("_task_resolution_failures", {}).get(task_id)
    )
    if declared and not resolution_failed:
        return "declared"
    if knowledge and resolution_failed:
        return "partial"
    if knowledge:
        return "contract-path"
    if resolution_failed:
        return "unresolved"
    return "undeclared"


def _note_reason(result: dict, context_node: dict) -> str:
    context_id = context_node["id"]
    scope = context_node.get("scope", [])
    if not scope:
        return "no-scope"
    if any(lint["context_node_id"] == context_id for lint in result["hub_lints"]):
        return "hub-withheld"
    if not _tasks_with_non_empty_touches(result):
        if any(omission["reason"] == "codegraph_unavailable" for omission in result["omissions"]):
            return "codegraph-unavailable"
        return "no-task-touches"
    return "scope-unresolved"


def _decision_id(*parts: str) -> str:
    name = "hive-manager:retrieval:" + ":".join(parts)
    return str(uuid.uuid5(DECISION_NAMESPACE, name))


def _existing_ledger_keys(ledger_path: Path) -> tuple[set[str], set[tuple[str, str, str]]]:
    decisions: set[str] = set()
    outcomes: set[tuple[str, str, str]] = set()
    for row in judgment_ledger.read_records([ledger_path]):
        if row.get("kind") == "decision":
            decisions.add(str(row.get("decision_id")))
        elif row.get("kind") == "outcome":
            outcomes.add(
                (str(row.get("decision_id")), str(row.get("label")), str(row.get("source")))
            )
    return decisions, outcomes


def _record_pair(
    *,
    ledger_path: Path,
    existing_decisions: set[str],
    existing_outcomes: set[tuple[str, str, str]],
    repo_name: str,
    session_id: str,
    ruleset: str,
    edge: dict,
    task_status: str,
    path_relevant: Optional[bool],
) -> None:
    subject = {
        "repo": repo_name,
        "session_id": session_id,
        "ruleset": ruleset,
        "task_id": edge["task_id"],
        "context_node_id": edge["context_node_id"],
    }
    note_id = _decision_id(
        repo_name,
        session_id,
        ruleset,
        "hive.retrieval.attach.note",
        edge["task_id"],
        edge["context_node_id"],
    )
    task_id = _decision_id(
        repo_name,
        session_id,
        ruleset,
        "hive.retrieval.attach.task",
        edge["task_id"],
        edge["context_node_id"],
    )
    if note_id not in existing_decisions:
        judgment_ledger.record_decision(
            "hive.retrieval.attach.note",
            subject,
            "code",
            {"attached": True, "rationale": edge["rationale"]},
            mode="shadow",
            decision_id=note_id,
            ledger=ledger_path,
        )
        existing_decisions.add(note_id)
    if task_id not in existing_decisions:
        judgment_ledger.record_decision(
            "hive.retrieval.attach.task",
            subject,
            "code",
            {"status": task_status},
            mode="shadow",
            decision_id=task_id,
            ledger=ledger_path,
        )
        existing_decisions.add(task_id)
    if path_relevant is None:
        return
    label = "path-relevant" if path_relevant else "path-miss"
    outcome_key = (note_id, label, "downstream")
    if outcome_key not in existing_outcomes:
        judgment_ledger.record_outcome(
            note_id,
            label,
            "downstream",
            note="current-copy knowledge; path misses are an upper bound",
            ledger=ledger_path,
        )
        existing_outcomes.add(outcome_key)


def _empty_scorecard() -> dict:
    return {
        "sessions": 0,
        "tasks": 0,
        "tasks_with_touches": 0,
        "zero_knowledge_tasks": 0,
        "tasks_with_knowledge": 0,
        "knowledge_pairs": 0,
        "path_relevant_pairs": 0,
        "path_miss_pairs": 0,
        "path_unscoped_pairs": 0,
        "production_attached": 0,
    }


def _empty_entry_comparison() -> dict:
    return {
        ruleset: {
            f"{entry}_{metric}": 0
            for entry in ("compose", "plan_ready")
            for metric in ENTRY_COMPARISON_METRICS
        }
        for ruleset in RULESET_ORDER[1:]
    }


def _tasks_with_non_empty_touches(result: dict) -> set[str]:
    return {
        task_id
        for task_id, paths in result["knowledge_attachment_touches"].items()
        if paths
    }


def _update_entry_comparison(
    comparison: dict,
    ruleset: str,
    compose_result: dict,
    plan_ready_result: dict,
) -> None:
    for entry, result in (
        ("compose", compose_result),
        ("plan_ready", plan_ready_result),
    ):
        attached = {edge["task_id"] for edge in result["knowledge_edges"]}
        comparison[ruleset][f"{entry}_tasks_with_touches"] += len(
            _tasks_with_non_empty_touches(result)
        )
        comparison[ruleset][f"{entry}_tasks_with_knowledge"] += len(attached)
        comparison[ruleset][f"{entry}_knowledge_pairs"] += len(
            result["knowledge_edges"]
        )


def _finalize_entry_comparison(comparison: dict) -> dict:
    for values in comparison.values():
        values["different"] = any(
            values[f"compose_{metric}"] != values[f"plan_ready_{metric}"]
            for metric in ENTRY_COMPARISON_METRICS
        )
    return comparison


def _finalize_scorecard(scorecard: dict) -> dict:
    tasks = scorecard["tasks"]
    path_total = scorecard["path_relevant_pairs"] + scorecard["path_miss_pairs"]
    scorecard["touch_coverage"] = (
        scorecard["tasks_with_touches"] / tasks if tasks else 0.0
    )
    scorecard["attachable_share"] = (
        scorecard["tasks_with_knowledge"] / tasks if tasks else 0.0
    )
    scorecard["path_miss_rate_upper_bound"] = (
        scorecard["path_miss_pairs"] / path_total if path_total else None
    )
    scorecard["path_miss_label"] = "upper bound (current knowledge copy)"
    return scorecard


def run_replay(
    sessions_roots: Iterable[Path],
    *,
    ledger_path: Path,
    stale_report: Path,
    session_store_root: Path,
    spot_check_csv: Optional[Path] = None,
) -> dict:
    roots = [Path(root).resolve() for root in sessions_roots]
    repos = [root.parent if root.name == ".hive-manager" else root for root in roots]
    repository_root = Path(__file__).resolve().parents[2]
    if any(_is_within(stale_report, repo) for repo in [*repos, repository_root]):
        raise ValueError("stale-scope report must be written outside every repository")
    if any(_is_within(ledger_path, repo) for repo in [*repos, repository_root]):
        raise ValueError("judgment ledger must be written outside every repository")
    spot_check_csv = (
        Path(spot_check_csv).resolve()
        if spot_check_csv is not None
        else stale_report.with_name("retrieval-spot-check.csv").resolve()
    )
    if any(_is_within(spot_check_csv, repo) for repo in [*repos, repository_root]):
        raise ValueError("spot-check CSV must be written outside every repository")

    existing_decisions, existing_outcomes = _existing_ledger_keys(ledger_path)
    report = {
        "rulesets": list(RULESET_ORDER),
        "entry_modes": COLUMN_ENTRY_MODES,
        "production_attached": 0,
        "entry_mode_comparison": _empty_entry_comparison(),
        "repositories": {},
        "named_sessions": [],
        "diff_unavailable": [],
        "spawn_context_errors": [],
        "knowledge_ack_errors": [],
        "delivery_coverage": _delivery_coverage(0, 0),
        "knowledge_ack_metrics": summarize_knowledge_acks([], []),
        "spot_check_csv": str(spot_check_csv),
        "path_miss_label": "upper bound (current knowledge copy)",
    }
    worker_spawns = 0
    planned_spawns = 0
    sampled_spawns = []
    ack_observations = []
    spot_check_rows = []
    stale_scope_rows = []
    unresolved_task_path_rows = []
    for sessions_root, repo in zip(roots, repos):
        repo_name = repo.name
        inventory, inventory_error = tracked_files(repo)
        scorecards = {ruleset: _empty_scorecard() for ruleset in RULESET_ORDER}
        entry_comparison = _empty_entry_comparison()
        repo_report = {
            "sessions_seen": 0,
            "parseable_sessions": 0,
            "pre_grammar_sessions": 0,
            "unparseable_sessions": 0,
            "inventory_error": inventory_error,
            "production_attached": 0,
            "spawn_context_errors": [],
            "knowledge_ack_errors": [],
            "delivery_coverage": _delivery_coverage(0, 0),
            "knowledge_ack_metrics": summarize_knowledge_acks([], []),
            "scorecards": scorecards,
        }
        repo_worker_spawns = 0
        repo_planned_spawns = 0
        repo_sampled_spawns = []
        repo_ack_observations = []
        for session in discover_sessions(sessions_root):
            repo_report["sessions_seen"] += 1
            spawn_contexts, spawn_errors = load_spawn_contexts(session)
            for error in spawn_errors:
                detail = {
                    "repo": repo_name,
                    "session_id": session.name,
                    "reason": error,
                }
                report["spawn_context_errors"].append(detail)
                repo_report["spawn_context_errors"].append(detail)
            for context in spawn_contexts:
                worker_spawns += 1
                repo_worker_spawns += 1
                plan_task_id = context.get("plan_task_id")
                if isinstance(plan_task_id, str) and plan_task_id.strip():
                    planned_spawns += 1
                    repo_planned_spawns += 1
                _record_spawn_context(
                    ledger_path=ledger_path,
                    existing_decisions=existing_decisions,
                    repo_name=repo_name,
                    session_id=session.name,
                    context=context,
                )
            content = (session / "plan.md").read_text(encoding="utf-8")
            classification, reason = _plan_classification(content)
            if classification != "parseable":
                key = "pre_grammar_sessions" if classification == "pre-grammar" else "unparseable_sessions"
                repo_report[key] += 1
                report["named_sessions"].append(
                    {"repo": repo_name, "session_id": session.name, "reason": reason}
                )
                (
                    session_sampled_spawns,
                    session_observations,
                    session_spot_checks,
                    ack_errors,
                ) = _evaluate_session_acks(
                    ledger_path=ledger_path,
                    existing_outcomes=existing_outcomes,
                    repo_name=repo_name,
                    session=session,
                    session_store_root=session_store_root,
                    contexts=spawn_contexts,
                    changed=None,
                )
                sampled_spawns.extend(session_sampled_spawns)
                repo_sampled_spawns.extend(session_sampled_spawns)
                ack_observations.extend(session_observations)
                repo_ack_observations.extend(session_observations)
                spot_check_rows.extend(session_spot_checks)
                for error in ack_errors:
                    detail = {
                        "repo": repo_name,
                        "session_id": session.name,
                        "reason": error,
                    }
                    report["knowledge_ack_errors"].append(detail)
                    repo_report["knowledge_ack_errors"].append(detail)
                continue
            repo_report["parseable_sessions"] += 1
            production_attached, production_error = observed_production_attachments(
                session_store_root, session.name
            )
            report["production_attached"] += production_attached
            repo_report["production_attached"] += production_attached
            for score in scorecards.values():
                score["production_attached"] += production_attached
            if production_error is not None:
                report["named_sessions"].append(
                    {
                        "repo": repo_name,
                        "session_id": session.name,
                        "reason": production_error,
                    }
                )
            changed, diff_reason = recover_changed_files(repo, session.name)
            if changed is None:
                report["diff_unavailable"].append(
                    {"repo": repo_name, "session_id": session.name, "reason": diff_reason}
                )
            (
                session_sampled_spawns,
                session_observations,
                session_spot_checks,
                ack_errors,
            ) = _evaluate_session_acks(
                ledger_path=ledger_path,
                existing_outcomes=existing_outcomes,
                repo_name=repo_name,
                session=session,
                session_store_root=session_store_root,
                contexts=spawn_contexts,
                changed=changed,
            )
            sampled_spawns.extend(session_sampled_spawns)
            repo_sampled_spawns.extend(session_sampled_spawns)
            ack_observations.extend(session_observations)
            repo_ack_observations.extend(session_observations)
            spot_check_rows.extend(session_spot_checks)
            for error in ack_errors:
                detail = {
                    "repo": repo_name,
                    "session_id": session.name,
                    "reason": error,
                }
                report["knowledge_ack_errors"].append(detail)
                repo_report["knowledge_ack_errors"].append(detail)
            matrix, comparisons = evaluate_replay_session(
                repo, session, inventory
            )
            for ruleset, (compose_result, plan_ready_result) in comparisons.items():
                _update_entry_comparison(
                    entry_comparison,
                    ruleset,
                    compose_result,
                    plan_ready_result,
                )
                _update_entry_comparison(
                    report["entry_mode_comparison"],
                    ruleset,
                    compose_result,
                    plan_ready_result,
                )
            plan, _diagnostics = parse_plan_markdown_with_diagnostics(content)
            task_ids = [task.id for task in plan.tasks if task.explicit_id]
            for ruleset, result in matrix.items():
                score = scorecards[ruleset]
                score["sessions"] += 1
                score["tasks"] += len(task_ids)
                touched = _tasks_with_non_empty_touches(result)
                attached = {edge["task_id"] for edge in result["knowledge_edges"]}
                score["tasks_with_touches"] += len(touched)
                score["zero_knowledge_tasks"] += len(set(task_ids) - attached)
                score["tasks_with_knowledge"] += len(attached)
                score["knowledge_pairs"] += len(result["knowledge_edges"])
                contexts = {node["id"]: node for node in result["context_nodes"]}
                missing_context_ids = sorted(
                    {
                        edge["context_node_id"]
                        for edge in result["knowledge_edges"]
                    }
                    - set(contexts)
                )
                if missing_context_ids:
                    raise AssertionError(
                        f"{ruleset} knowledge edges target missing context nodes: "
                        + ", ".join(missing_context_ids[:5])
                    )
                for omission in result["omissions"]:
                    detail = omission.get("detail", "")
                    row = {
                        "repo": repo_name,
                        "session_id": session.name,
                        "ruleset": ruleset,
                        "count": omission["count"],
                        "detail": detail,
                    }
                    if detail in STALE_SCOPE_OMISSION_DETAILS:
                        stale_scope_rows.append(row)
                    elif detail == TASK_PATH_UNRESOLVED_DETAIL:
                        unresolved_task_path_rows.append(row)
                unattached = [
                    _note_reason(result, context)
                    for context in result["context_nodes"]
                    if context["id"] not in {
                        edge["context_node_id"]
                        for edge in result["knowledge_edges"]
                    }
                ]
                unexpected = sorted(set(unattached) - set(NOTE_REASONS))
                if unexpected:
                    raise AssertionError(f"unexpected unattached reasons: {unexpected}")
                score["unattached_reasons"] = dict(
                    sorted(
                        (
                            Counter(score.get("unattached_reasons", {}))
                            + Counter(unattached)
                        ).items()
                    )
                )
                for edge in result["knowledge_edges"]:
                    context = contexts[edge["context_node_id"]]
                    relevant = _path_relevance(context, changed)
                    if not context.get("scope", []):
                        score["path_unscoped_pairs"] += 1
                    elif relevant is not None:
                        score[
                            "path_relevant_pairs" if relevant else "path_miss_pairs"
                        ] += 1
                    status = _task_status(result, edge["task_id"])
                    if status not in TASK_STATUSES:
                        raise AssertionError(f"unexpected task status {status}")
                    _record_pair(
                        ledger_path=ledger_path,
                        existing_decisions=existing_decisions,
                        existing_outcomes=existing_outcomes,
                        repo_name=repo_name,
                        session_id=session.name,
                        ruleset=ruleset,
                        edge=edge,
                        task_status=status,
                        path_relevant=relevant,
                    )
        repo_report["scorecards"] = {
            ruleset: _finalize_scorecard(scorecards[ruleset]) for ruleset in RULESET_ORDER
        }
        repo_report["entry_mode_comparison"] = _finalize_entry_comparison(
            entry_comparison
        )
        repo_report["delivery_coverage"] = _delivery_coverage(
            repo_worker_spawns, repo_planned_spawns
        )
        repo_report["knowledge_ack_metrics"] = summarize_knowledge_acks(
            repo_sampled_spawns, repo_ack_observations
        )
        report["repositories"][repo_name] = repo_report

    report["delivery_coverage"] = _delivery_coverage(
        worker_spawns, planned_spawns
    )
    report["knowledge_ack_metrics"] = summarize_knowledge_acks(
        sampled_spawns, ack_observations
    )
    report["entry_mode_comparison"] = _finalize_entry_comparison(
        report["entry_mode_comparison"]
    )
    stale_report.parent.mkdir(parents=True, exist_ok=True)
    stale_report.write_text(
        json.dumps(
            {
                "stale_scope_omissions": stale_scope_rows,
                "unresolved_task_path_omissions": unresolved_task_path_rows,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
        newline="\n",
    )
    write_spot_check_csv(spot_check_csv, spot_check_rows)
    return report


def _default_stale_report() -> Path:
    return Path(tempfile.gettempdir()) / "hive-retrieval-stale-scope.json"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sessions", action="append", required=True, type=Path)
    parser.add_argument("--ledger", type=Path)
    parser.add_argument("--session-store", type=Path)
    parser.add_argument("--spot-check-csv", type=Path)
    parser.add_argument("--stale-report", type=Path, default=_default_stale_report())
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    arguments = build_parser().parse_args(argv)
    ledger_path = resolve_ledger_path(arguments.ledger).resolve()
    report = run_replay(
        arguments.sessions,
        ledger_path=ledger_path,
        stale_report=arguments.stale_report.resolve(),
        session_store_root=resolve_session_store_root(arguments.session_store).resolve(),
        spot_check_csv=(
            arguments.spot_check_csv.resolve()
            if arguments.spot_check_csv is not None
            else None
        ),
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
