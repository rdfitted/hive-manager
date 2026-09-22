"""Deterministic retrieval rules used by the offline replay harness."""

from __future__ import annotations

import json
import math
import os
import re
import string
import uuid
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path, PureWindowsPath
from typing import Any, Optional

from plan_grammar import PlanTask, SessionPlan, parse_plan_markdown_with_diagnostics, rust_lines


RUST_RULESET = "current"

MAX_CONTEXT_SUMMARY_CHARS = 240
MAX_DERIVED_CONTEXT_NODES = 128
MAX_CONTEXT_SCOPES_PER_GOTCHA = 16
MAX_CONTEXT_SCOPE_CHARS = 256
MAX_TOUCH_MODULES_PER_TASK = 256
MAX_MODULE_PATH_CHARS = 512
MAX_FILE_INVENTORY_ENTRIES = 50_000
MAX_FILE_INVENTORY_PATH_CHARS = 512
MAX_OMISSION_EXAMPLES = 5
ANTI_HUB_TASK_FRACTION = 0.75
ANTI_HUB_MIN_TASKS = 2

OMISSION_DETAILS = {
    "codegraph_unavailable": "codegraph output was unavailable, so repository relationships are incomplete",
    "project_knowledge_unavailable": "project knowledge was unavailable, so context relationships are incomplete",
    "source_unreadable": "a graph source could not be read, so derived relationships are incomplete",
    "resolution_incomplete": "one or more graph references could not be resolved",
}


def _ascii_lower(value: str) -> str:
    return value.translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"))


def stable_hash(value: bytes) -> str:
    result = 0xCBF29CE484222325
    for byte in value:
        result ^= byte
        result = (result * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{result:016x}"


def _omission(reason: str, count: int, examples: list[str], detail: Optional[str] = None) -> dict[str, Any]:
    return {
        "reason": reason,
        "count": count,
        "detail": detail or OMISSION_DETAILS[reason],
        "examples": examples,
    }


@dataclass
class TaskNode:
    id: str
    title: str
    inputs: list[str]
    outputs: list[str]
    acceptance: list[str]


@dataclass
class TaskGraph:
    tasks: list[TaskNode]
    depends_on_edges: list[dict[str, Any]]
    omissions: list[dict[str, Any]] = field(default_factory=list)
    touches_edges: list[dict[str, Any]] = field(default_factory=list)
    informs_edges: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class Gotcha:
    id: str
    scope: list[str]
    summary: str
    source_ref: str
    fingerprint_ref: str
    source_hash: str


def task_graph_from_plan(plan: SessionPlan) -> TaskGraph:
    has_explicit_graph = any(task.explicit_id for task in plan.tasks)
    seen: set[str] = set()
    duplicates: list[str] = []
    graph_tasks: list[PlanTask] = []
    for task in plan.tasks:
        if has_explicit_graph and not task.explicit_id:
            continue
        if task.id in seen:
            duplicates.append(task.id)
            continue
        seen.add(task.id)
        graph_tasks.append(task)
    nodes = [TaskNode(task.id, task.title, task.inputs[:], task.outputs[:], task.acceptance[:]) for task in graph_tasks]
    edges = [
        {"source": dependency, "target": task.id, "kind": "depends_on", "provenance": "planner", "rationale": None}
        for task in graph_tasks
        for dependency in task.depends_on
    ]
    omissions: list[dict[str, Any]] = []
    if duplicates:
        duplicates.sort()
        omissions.append(_omission(
            "resolution_incomplete",
            len(duplicates),
            [f"duplicate task {task_id} was omitted after its first declaration" for task_id in duplicates],
        ))
    unrecognized_bindings = [task for task in graph_tasks if task.assignee is not None and not task.assignee_recognized]
    if unrecognized_bindings:
        omissions.append(_omission(
            "resolution_incomplete",
            len(unrecognized_bindings),
            [f"task {task.id} preserved unrecognized principal binding {task.assignee or 'unassigned'}" for task in unrecognized_bindings],
        ))
    unrecognized_tiers = [task for task in graph_tasks if not task.tier_recognized]
    if unrecognized_tiers:
        omissions.append(_omission(
            "resolution_incomplete",
            len(unrecognized_tiers),
            [f"task {task.id} preserved unrecognized tier {task.tier_source or '<missing>'} as medium" for task in unrecognized_tiers],
        ))
    return TaskGraph(nodes, edges, omissions)


def _is_absolute_like_rust(value: str) -> bool:
    if os.name == "nt":
        return PureWindowsPath(value).is_absolute()
    return Path(value).is_absolute()


def normalize_artifact_path(value: str) -> Optional[str]:
    normalized = value.replace("\\", "/")
    if _is_absolute_like_rust(normalized):
        return None
    components = [component for component in normalized.split("/") if component and component != "."]
    if not components or ".." in components:
        return None
    return _ascii_lower("/".join(components))


def _aliases(module_id: str, path: str) -> set[str]:
    aliases = {path}
    if "." in path:
        aliases.add(path.rsplit(".", 1)[0])
    normalized_id = _ascii_lower(module_id.replace("\\", "/"))
    if not _is_absolute_like_rust(normalized_id):
        aliases.add(normalized_id)
    return aliases


def _explicit_touch_intents(node: TaskNode) -> list[Optional[str]]:
    intents: list[Optional[str]] = []
    for value in node.inputs + node.outputs + node.acceptance:
        trimmed = value.strip()
        lowered = _ascii_lower(trimmed)
        raw = next((trimmed[len(prefix) :] for prefix in ("touch:", "file:", "module:") if lowered.startswith(prefix)), None)
        if raw is None:
            continue
        normalized = raw.strip().strip("`\"',;")
        if _ascii_lower(normalized) == "none":
            intents.append(None)
            continue
        path = normalize_artifact_path(normalized)
        if path is not None:
            intents.append(path)
    return intents


def _resolve_intent(modules: list[str], aliases: dict[str, set[str]], intent: str) -> Optional[list[str]]:
    if intent in modules:
        return [intent]
    if intent in aliases:
        matches = sorted(aliases[intent])
        return matches if len(matches) == 1 else None
    prefix = f"{intent}/"
    descendants = [module for module in modules if module.startswith(prefix)][: MAX_TOUCH_MODULES_PER_TASK + 1]
    return descendants if 0 < len(descendants) <= MAX_TOUCH_MODULES_PER_TASK else None


def _source_language(path: str) -> Optional[str]:
    if "." not in path:
        return None
    extension = path.rsplit(".", 1)[1]
    if extension == "rs": return "rust"
    if extension in {"py", "pyi"}: return "python"
    if extension in {"ts", "tsx", "js", "jsx", "mjs", "cjs"}: return "typescript/javascript"
    if extension == "svelte": return "svelte"
    if extension == "go": return "go"
    if extension in {"java", "kt", "kts"}: return "jvm"
    if extension == "cs": return "csharp"
    if extension in {"c", "cc", "cpp", "h", "hpp"}: return "c/cpp"
    return None


def _load_artifact(root: Path) -> Optional[tuple[list[str], dict[str, set[str]], set[str]]]:
    artifact_path = root / "codegraph.json"
    if not artifact_path.is_file():
        return None
    raw = json.loads(artifact_path.read_text(encoding="utf-8"))
    modules: set[str] = set()
    aliases: dict[str, set[str]] = {}
    for module_id in sorted(raw.get("nodes", {})):
        path = normalize_artifact_path(raw["nodes"][module_id]["path"])
        if path is None or len(path) > MAX_MODULE_PATH_CHARS or _path_is_excluded(path):
            continue
        modules.add(path)
        for alias in _aliases(module_id, path):
            aliases.setdefault(alias, set()).add(path)
    sorted_modules = sorted(modules)
    languages = {language for module in sorted_modules if (language := _source_language(module)) is not None}
    return sorted_modules, aliases, languages


def _path_is_excluded(path: str) -> bool:
    excluded = {".hive-manager", ".hive-fusion", ".hive-debate", ".git", ".hg", ".svn", ".claude", ".codex", ".agents", ".svelte-kit", ".next", ".turbo", ".cache", ".vite", "node_modules", "target", "dist", "build", "coverage", "package", "out"}
    return any(component in excluded for component in path.split("/"))


def _derive_codegraph(graph: TaskGraph, artifact: Optional[tuple[list[str], dict[str, set[str]], set[str]]]) -> tuple[dict[str, list[str]], list[str], bool]:
    if artifact is None:
        graph.omissions.append(_omission("codegraph_unavailable", 1, ["touches-resolver"]))
        return {}, [], False
    modules, aliases, covered_languages = artifact
    touches: dict[str, list[str]] = {}
    for node in graph.tasks:
        intents = _explicit_touch_intents(node)
        if not intents:
            continue
        resolved: set[str] = set()
        complete = True
        for intent in intents:
            if intent is None:
                continue
            matches = _resolve_intent(modules, aliases, intent)
            if matches is None:
                complete = False
            else:
                resolved.update(matches)
        if complete and len(resolved) <= MAX_TOUCH_MODULES_PER_TASK:
            touches[node.id] = sorted(resolved)
    touches = {task_id: touches[task_id] for task_id in sorted(touches)}
    unresolved = [node.id for node in graph.tasks if node.id not in touches]
    task_by_id = {node.id: node for node in sorted(graph.tasks, key=lambda node: node.id)}
    undeclared: list[str] = []
    uncovered: list[str] = []
    uncovered_languages: set[str] = set()
    for task_id in unresolved:
        node = task_by_id[task_id]
        intents = _explicit_touch_intents(node)
        if not intents:
            undeclared.append(task_id)
            continue
        for intent in intents:
            if intent is not None and (language := _source_language(intent)) is not None:
                uncovered_languages.add(language)
        uncovered.append(task_id)
    if undeclared:
        graph.omissions.append(_omission(
            "resolution_incomplete", len(undeclared), undeclared[:MAX_OMISSION_EXAMPLES],
            "codegraph artifact was available, but explicit task touch intent was not declared",
        ))
    if uncovered:
        languages = ", ".join(sorted(uncovered_languages)) if uncovered_languages else "unknown"
        graph.omissions.append(_omission(
            "resolution_incomplete", len(uncovered), uncovered[:MAX_OMISSION_EXAMPLES],
            f"codegraph artifact was available but did not cover or resolve declared language(s): {languages}",
        ))
    for task_id, task_modules in touches.items():
        for module in task_modules:
            graph.touches_edges.append({
                "source": task_id,
                "target": f"codegraph::module::{uuid.uuid5(uuid.NAMESPACE_URL, f'hive-manager:codegraph:{module}')}",
                "kind": "touches",
                "provenance": "codegraph",
                "rationale": f"explicit task intent resolved to module {module}",
            })
    return touches, unresolved, True


def normalize_scope(scope: str) -> str:
    normalized = scope.strip().strip("`\"',").replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return _ascii_lower(normalized.strip("/"))


def _bound_summary(summary: str) -> str:
    if len(summary) <= MAX_CONTEXT_SUMMARY_CHARS:
        return summary
    return summary[: MAX_CONTEXT_SUMMARY_CHARS - 1] + "…"


def _code_spans(line: str) -> list[str]:
    return line.split("`")[1::2]


def _markdown_scope(body: list[str]) -> list[str]:
    scopes: set[str] = set()
    for line in body:
        lower = _ascii_lower(line)
        explicit = "**scope**" in lower or "**files**" in lower or _ascii_lower(line.lstrip()).startswith("scope:")
        if not explicit:
            continue
        values = _code_spans(line)
        if not values:
            plain = line.replace("**", "")
            if ":" in plain:
                values = plain.split(":", 1)[1].split(",")
        scopes.update(normalize_scope(value) for value in values)
    return sorted(scope for scope in scopes if scope)


def _strip_markdown_label(value: str) -> str:
    value = value.replace("**", "")
    return value.lstrip(string.punctuation).strip()


def _markdown_summary(heading: str, body: list[str]) -> str:
    for line in body:
        trimmed = line.strip().lstrip("-").strip()
        lowered = _ascii_lower(trimmed)
        if not trimmed or trimmed.startswith("```") or "-> global:" in trimmed or "**scope**" in lowered or "**files**" in lowered:
            continue
        return _bound_summary(f"{heading}: {_strip_markdown_label(trimmed)}")
    return _bound_summary(heading)


def _global_cross_ref(line: str) -> Optional[str]:
    marker = "-> global:"
    position = line.find(marker)
    if position < 0:
        return None
    tail = line[position + len(marker) :].strip()
    if "](" in tail:
        raw = tail.split("](", 1)[1]
        if ")" not in raw:
            return None
        raw = raw.split(")", 1)[0]
    else:
        raw = tail
    for category in ("agents/", "operations/", "patterns/", "practices/", "research/", "tools/"):
        start = raw.find(category)
        if start >= 0:
            rest = raw[start:]
            end = rest.find(".md")
            return None if end < 0 else rest[: end + 3]
    return None


def _parse_markdown(filename: str, content: str, source_hash: str) -> list[Gotcha]:
    gotchas: list[Gotcha] = []
    heading = ""
    start_line = 1
    body: list[str] = []
    in_fence = False

    def flush() -> None:
        if not heading or _ascii_lower(heading) in {"template", "bugs"} or all(not line.strip() for line in body):
            return
        scope = _markdown_scope(body)
        summary = _markdown_summary(heading, body)
        if not summary:
            return
        source_ref = f".ai-docs/{filename}#L{start_line}"
        gotchas.append(Gotcha(stable_hash(f"{source_ref}:{heading}".encode()), scope[:], summary, source_ref, f".ai-docs/{filename}", source_hash))
        for line in body:
            global_ref = _global_cross_ref(line)
            if global_ref is not None:
                source_ref = f"global:{global_ref}"
                gotchas.append(Gotcha(stable_hash(f"{source_ref}:{heading}".encode()), scope[:], _bound_summary(f"{heading}: institutional context"), source_ref, f".ai-docs/{filename}", source_hash))

    for index, line in enumerate(rust_lines(content)):
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        trimmed = line.strip()
        hashes = len(trimmed) - len(trimmed.lstrip("#"))
        next_heading = trimmed[hashes:].strip() if 2 <= hashes <= 4 else None
        if next_heading is not None:
            flush()
            heading, start_line, body = next_heading, index + 1, []
        elif heading:
            body.append(line)
    flush()
    return gotchas


def _load_knowledge(root: Path) -> tuple[list[Gotcha], list[dict[str, Any]], bool]:
    ai_docs = root / ".ai-docs"
    if not ai_docs.is_dir():
        return [], [_omission("project_knowledge_unavailable", 1, [str(ai_docs)])], False
    omissions: list[dict[str, Any]] = []
    gotchas: list[Gotcha] = []
    curated_line_limit: Optional[int] = None
    curation_content = (ai_docs / "curation-state.json").read_text(encoding="utf-8")
    try:
        value = json.loads(curation_content)
        line = value.get("last_curated_line")
        if isinstance(line, int) and not isinstance(line, bool) and line >= 0:
            curated_line_limit = line
        else:
            omissions.append(_omission("resolution_incomplete", 1, [".ai-docs/curation-state.json:last_curated_line"]))
    except json.JSONDecodeError as error:
        omissions.append(_omission("source_unreadable", 1, [f"{ai_docs / 'curation-state.json'}: {error}"]))

    for filename in ("project-dna.md", "bug-patterns.md", "learnings.jsonl"):
        content = (ai_docs / filename).read_text(encoding="utf-8")
        source_hash = stable_hash(content.encode())
        if not filename.endswith(".jsonl"):
            gotchas.extend(_parse_markdown(filename, content, source_hash))
        elif curated_line_limit is not None:
            curated_hash = stable_hash(f"{source_hash}:last-curated-line={curated_line_limit}".encode())
            for index, line in enumerate(rust_lines(content)[:curated_line_limit]):
                if not line.strip():
                    continue
                try:
                    value = json.loads(line)
                except json.JSONDecodeError as error:
                    omissions.append(_omission("source_unreadable", 1, [f".ai-docs/{filename}#L{index + 1}: {error}"]))
                    continue
                insight = value.get("insight")
                if not isinstance(insight, str):
                    continue
                raw_scope = value.get("files_touched")
                scope = [
                    normalize_scope(item)
                    for item in raw_scope
                    if isinstance(item, str)
                ] if isinstance(raw_scope, list) else []
                scope = [item for item in scope if item]
                source_ref = f".ai-docs/{filename}#L{index + 1}"
                stable_id = value.get("id") if isinstance(value.get("id"), str) else stable_hash(line.encode())
                gotchas.append(Gotcha(stable_id, scope, _bound_summary(insight), source_ref, f".ai-docs/{filename}", curated_hash))

    dropped_count = 0
    dropped_sources: set[str] = set()
    for gotcha in gotchas:
        original = sorted(set(gotcha.scope))
        gotcha.scope = [scope for scope in original if len(scope) <= MAX_CONTEXT_SCOPE_CHARS][:MAX_CONTEXT_SCOPES_PER_GOTCHA]
        dropped = len(original) - len(gotcha.scope)
        if dropped:
            dropped_count += dropped
            dropped_sources.add(gotcha.source_ref)
    if dropped_count:
        omissions.append(_omission("resolution_incomplete", dropped_count, sorted(dropped_sources)[:MAX_OMISSION_EXAMPLES]))
    gotchas.sort(key=lambda gotcha: (not bool(gotcha.scope), gotcha.source_ref, gotcha.id))
    unique: list[Gotcha] = []
    seen: set[str] = set()
    for gotcha in gotchas:
        if gotcha.id not in seen:
            seen.add(gotcha.id)
            unique.append(gotcha)
    if len(unique) > MAX_DERIVED_CONTEXT_NODES:
        omitted = len(unique) - MAX_DERIVED_CONTEXT_NODES
        unique = unique[:MAX_DERIVED_CONTEXT_NODES]
        omissions.append(_omission("resolution_incomplete", omitted, [f"context node cap {MAX_DERIVED_CONTEXT_NODES}"]))
    return unique, omissions, True


def _scope_intersects(scope: list[str], touches: list[str]) -> bool:
    return any(
        scope_item == "*" or any(
            scope_item == (touch := normalize_scope(raw_touch))
            or touch.startswith(f"{scope_item}/")
            or scope_item.startswith(f"{touch}/")
            for raw_touch in touches
        )
        for scope_item in scope
    )


def _fixture_entry(root: Path) -> str:
    entry_path = root / "entry.txt"
    # Historical replay materializations are not golden fixtures and model the
    # base compose rule unless their caller explicitly supplies an entry.
    if not entry_path.is_file():
        return "compose"
    entry = entry_path.read_text(encoding="utf-8").strip()
    if entry not in {"compose", "plan-ready"}:
        raise ValueError("fixture entry must be compose or plan-ready")
    return entry


def run_current(root: Path) -> dict[str, Any]:
    """Run the base Rust plan -> touches -> knowledge pipeline for one fixture."""

    content = (root / "plan.md").read_text(encoding="utf-8")
    plan, _diagnostics = parse_plan_markdown_with_diagnostics(content)
    graph = task_graph_from_plan(plan)
    entry = _fixture_entry(root)
    if entry == "compose":
        touches, unresolved, touches_available = _derive_codegraph(graph, _load_artifact(root))
    else:
        touches, unresolved, touches_available = {}, [], False
    gotchas, load_omissions, knowledge_available = _load_knowledge(root)
    graph.omissions.extend(load_omissions)
    hub_lints: list[dict[str, Any]] = []
    if knowledge_available and not touches_available:
        graph.omissions.append(_omission("codegraph_unavailable", 1, ["touches-resolver"]))
    elif knowledge_available:
        if unresolved:
            graph.omissions.append(_omission("resolution_incomplete", len(unresolved), unresolved[:MAX_OMISSION_EXAMPLES]))
        task_ids = [node.id for node in graph.tasks]
        candidates: dict[str, list[str]] = {}
        for gotcha in gotchas:
            context_id = f"context::knowledge::{gotcha.id}"
            for task_id in task_ids:
                if task_id in touches and _scope_intersects(gotcha.scope, touches[task_id]):
                    candidates.setdefault(context_id, []).append(task_id)
        for gotcha in gotchas:
            context_id = f"context::knowledge::{gotcha.id}"
            linked = candidates.pop(context_id, [])
            if "*" in gotcha.scope and len(task_ids) >= ANTI_HUB_MIN_TASKS:
                linked = task_ids[:]
            fraction = len(linked) / len(task_ids) if task_ids else 0.0
            if len(task_ids) >= ANTI_HUB_MIN_TASKS and fraction >= ANTI_HUB_TASK_FRACTION:
                hub_lints.append({
                    "context_node_id": context_id,
                    "linked_task_ids": linked,
                    "reason": "context applies to a high fraction of tasks; move standing guidance to the role prompt or narrow its scope",
                })
                continue
            for task_id in linked:
                graph.informs_edges.append({
                    "source": context_id,
                    "target": task_id,
                    "kind": "informs",
                    "provenance": "knowledge",
                    "rationale": "task touches a module in this gotcha's scope",
                })
    unique_omissions: list[dict[str, Any]] = []
    for omission in graph.omissions:
        if omission not in unique_omissions:
            unique_omissions.append(omission)
    return {
        "entry": entry,
        "parsed_note_count": len(gotchas),
        "declared_touches": touches,
        "knowledge_attachment_touches": touches,
        "knowledge_edges": [
            {"task_id": edge["target"], "context_node_id": edge["source"], "rationale": edge["rationale"]}
            for edge in graph.informs_edges
        ],
        "context_nodes": [
            {
                "id": f"context::knowledge::{gotcha.id}",
                "title": gotcha.summary,
                "summary": gotcha.summary,
                "scope": gotcha.scope,
                "parameters": {
                    "fingerprint_ref": gotcha.fingerprint_ref,
                    "scope": ",".join(gotcha.scope),
                    "source_hash": gotcha.source_hash,
                    "source_ref": gotcha.source_ref,
                    "summary": gotcha.summary,
                },
            }
            for gotcha in gotchas
        ],
        "omissions": unique_omissions,
        "hub_lints": hub_lints,
    }


_LINE_RANGE = re.compile(r":\d+(?:-\d+)?$")
_CONTRACT_SPLIT = re.compile(r"[\s,;()\[\]{}<>]+")


def _strip_path_decoration(value: str) -> str:
    value = value.strip().strip("`\"'")
    value = value.rstrip(".!?;,)")
    return _LINE_RANGE.sub("", value)


def _is_path_like_token(value: str) -> bool:
    if "/" in value or "\\" in value:
        return True
    stripped = _strip_path_decoration(value)
    final_component = stripped.replace("\\", "/").rsplit("/", 1)[-1]
    if "." not in final_component:
        return False
    stem, extension = final_component.rsplit(".", 1)
    return bool(stem and extension)


def resolve_path_intent(
    intent: str, candidates: list[str], *, allow_parent: bool = False
) -> tuple[Optional[str], Optional[str], Optional[str]]:
    """Mirror the ACKed exact -> basename -> suffix -> parent design order."""

    decorated = _strip_path_decoration(intent)
    normalized = normalize_artifact_path(decorated)
    if normalized is None:
        return None, None, "stale"
    candidate_set = set(candidates)
    if normalized in candidate_set:
        return normalized, "exact", None
    if "/" not in normalized:
        basename = sorted(path for path in candidates if path.rsplit("/", 1)[-1] == normalized)
        if len(basename) == 1:
            return basename[0], "unique-basename", None
        if len(basename) > 1:
            return None, None, "ambiguous"
    else:
        suffix = sorted(
            path
            for path in candidates
            if path == normalized or path.endswith(f"/{normalized}")
        )
        if len(suffix) == 1:
            return suffix[0], "path-suffix", None
        if len(suffix) > 1:
            return None, None, "ambiguous"
    if allow_parent and "/" in normalized:
        components = normalized.split("/")[:-1]
        while components:
            parent = "/".join(components)
            descendants = [
                path for path in candidates if path.startswith(f"{parent}/")
            ][: MAX_TOUCH_MODULES_PER_TASK + 1]
            if 0 < len(descendants) <= MAX_TOUCH_MODULES_PER_TASK:
                return parent, "parent-directory", None
            if len(descendants) > MAX_TOUCH_MODULES_PER_TASK:
                break
            components.pop()
    return None, None, "stale"


def _candidate_inventory(
    root: Path,
) -> tuple[Optional[list[str]], bool, list[dict[str, Any]]]:
    artifact = _load_artifact(root)
    if artifact is not None:
        return artifact[0], False, []
    inventory_path = root / "files.txt"
    try:
        content = inventory_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        detail = "tracked file inventory was not valid UTF-8"
        return None, True, [_omission("resolution_incomplete", 1, ["git ls-files"], detail)]
    except OSError:
        detail = "tracked file inventory was unavailable"
        return None, True, [_omission("resolution_incomplete", 1, ["git ls-files"], detail)]

    inventory: set[str] = set()
    for index, raw in enumerate(rust_lines(content)):
        if index >= MAX_FILE_INVENTORY_ENTRIES:
            detail = "tracked file inventory exceeded the entry limit"
            return None, True, [_omission("resolution_incomplete", 1, ["git ls-files"], detail)]
        slash_normalized = raw.replace("\\", "/")
        normalized = normalize_scope(raw)
        invalid = (
            not normalized
            or len(normalized) > MAX_FILE_INVENTORY_PATH_CHARS
            or slash_normalized.startswith("/")
            or (len(slash_normalized) > 1 and slash_normalized[1] == ":")
            or ".." in normalized.split("/")
            or _is_absolute_like_rust(normalized)
        )
        if invalid:
            detail = "tracked file inventory contained an invalid path"
            return None, True, [_omission("resolution_incomplete", 1, ["git ls-files"], detail)]
        inventory.add(normalized)
    return sorted(inventory), True, []


def _contract_intents(
    node: TaskNode, *, include_harvested: bool = True
) -> list[tuple[str, str]]:
    declared_intents: list[tuple[str, str]] = []
    harvested_intents: set[tuple[str, str]] = set()
    for value in node.inputs + node.outputs + node.acceptance:
        trimmed = value.strip()
        lowered = _ascii_lower(trimmed)
        declared = next(
            (trimmed[len(prefix) :] for prefix in ("touch:", "file:", "module:") if lowered.startswith(prefix)),
            None,
        )
        if declared is not None:
            if _ascii_lower(declared.strip()) != "none":
                declared_intents.append((declared, "declared-scope"))
            continue
        if not include_harvested:
            continue
        for token in _CONTRACT_SPLIT.split(value):
            token = _strip_path_decoration(token)
            if not token or (
                token.startswith(":") and _LINE_RANGE.fullmatch(token[1:]) is not None
            ):
                continue
            if "://" in token:
                continue
            lowered_token = _ascii_lower(token)
            if any(lowered_token.startswith(prefix) for prefix in ("touch:", "file:", "module:")):
                continue
            final_component = token.replace("\\", "/").rsplit("/", 1)[-1]
            path_like = "/" in token or "\\" in token or (
                "." in final_component
                and bool(final_component.split(".", 1)[0])
                and bool(final_component.rsplit(".", 1)[-1])
            )
            if path_like:
                normalized = normalize_artifact_path(token)
                if normalized is not None:
                    harvested_intents.add((normalized, "contract-path"))
    return declared_intents + sorted(harvested_intents)


def _knowledge_touches(
    graph: TaskGraph,
    declared_touches: dict[str, list[str]],
    candidates: list[str],
    fallback: bool,
    *,
    include_harvested: bool = True,
) -> tuple[
    dict[str, list[str]],
    dict[str, dict[str, set[str]]],
    list[dict[str, Any]],
]:
    touches = {task_id: paths[:] for task_id, paths in declared_touches.items()}
    provenance: dict[str, dict[str, set[str]]] = {
        task_id: {path: {"declared-scope"} for path in paths}
        for task_id, paths in declared_touches.items()
    }
    failures: dict[str, set[str]] = {}
    for node in sorted(graph.tasks, key=lambda item: item.id):
        resolved = set(touches.get(node.id, []))
        path_labels = provenance.setdefault(node.id, {})
        for raw, source in _contract_intents(
            node, include_harvested=include_harvested
        ):
            path, match_type, failure = resolve_path_intent(
                raw, candidates, allow_parent=True
            )
            if path is None:
                detail = (
                    "knowledge path resolution was ambiguous"
                    if failure == "ambiguous"
                    else "knowledge path resolution found no tracked path"
                )
                failures.setdefault(detail, set()).add(
                    f"{node.id}: {_strip_path_decoration(raw)}"
                )
                continue
            if path not in resolved and len(resolved) >= MAX_TOUCH_MODULES_PER_TASK:
                failures.setdefault("knowledge touch limit was reached", set()).add(
                    f"{node.id}: {_strip_path_decoration(raw)}"
                )
                continue
            resolved.add(path)
            labels = path_labels.setdefault(path, set())
            labels.add(source)
            if fallback:
                labels.add("fallback")
            if match_type == "parent-directory":
                labels.add("parent-directory")
        if resolved:
            touches[node.id] = sorted(resolved)
        elif not path_labels:
            provenance.pop(node.id, None)
    omissions = []
    for detail, examples in sorted(failures.items()):
        omissions.append(
            _omission(
                "resolution_incomplete",
                len(examples),
                sorted(examples)[:MAX_OMISSION_EXAMPLES],
                detail,
            )
        )
    return touches, provenance, omissions


def _inferred_scope_candidates(
    root: Path, candidates: list[str]
) -> tuple[
    dict[str, list[str]],
    dict[str, dict[str, str]],
    list[dict[str, Any]],
]:
    scopes: dict[str, list[str]] = {}
    provenance: dict[str, dict[str, str]] = {}
    failures: dict[str, set[str]] = {}
    for filename in ("project-dna.md", "bug-patterns.md"):
        path = root / ".ai-docs" / filename
        content = path.read_text(encoding="utf-8")
        heading = ""
        start_line = 1
        body: list[str] = []

        def flush() -> None:
            if not heading or _ascii_lower(heading) in {"template", "bugs"}:
                return
            explicit = any(
                "**scope**" in _ascii_lower(line)
                or "**files**" in _ascii_lower(line)
                or _ascii_lower(line.lstrip()).startswith("scope:")
                or _ascii_lower(line.lstrip()).startswith("files:")
                for line in body
            )
            if explicit:
                return
            source_ref = f".ai-docs/{filename}#L{start_line}"
            gotcha_id = stable_hash(f"{source_ref}:{heading}".encode())
            resolved: dict[str, str] = {}
            for line in body:
                for raw in _code_spans(line):
                    if not _is_path_like_token(raw):
                        continue
                    path_value, match_type, failure = resolve_path_intent(raw, candidates)
                    if path_value is None:
                        detail = (
                            "inferred knowledge scope was ambiguous"
                            if failure == "ambiguous"
                            else "inferred knowledge scope found no tracked path"
                        )
                        failures.setdefault(detail, set()).add(
                            f"{source_ref}: {_strip_path_decoration(raw)}"
                        )
                        continue
                    if (
                        path_value not in resolved
                        and len(resolved) >= MAX_CONTEXT_SCOPES_PER_GOTCHA
                    ):
                        failures.setdefault(
                            "inferred knowledge scope exceeded the scope limit", set()
                        ).add(f"{source_ref}: {_strip_path_decoration(raw)}")
                        continue
                    resolved[path_value] = f"inferred-scope:{match_type}"
            if resolved:
                gotcha_ids = [gotcha_id]
                for line in body:
                    global_ref = _global_cross_ref(line)
                    if global_ref is not None:
                        global_source_ref = f"global:{global_ref}"
                        gotcha_ids.append(
                            stable_hash(f"{global_source_ref}:{heading}".encode())
                        )
                for target_id in gotcha_ids:
                    scopes[target_id] = sorted(resolved)
                    provenance[target_id] = dict(sorted(resolved.items()))

        in_fence = False
        for index, line in enumerate(rust_lines(content)):
            if line.lstrip().startswith("```"):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            trimmed = line.strip()
            hashes = len(trimmed) - len(trimmed.lstrip("#"))
            if 2 <= hashes <= 4:
                flush()
                heading, start_line, body = trimmed[hashes:].strip(), index + 1, []
            elif heading:
                body.append(line)
        flush()
    omissions = [
        _omission(
            "resolution_incomplete",
            len(examples),
            sorted(examples)[:MAX_OMISSION_EXAMPLES],
            detail,
        )
        for detail, examples in sorted(failures.items())
    ]
    return scopes, provenance, omissions


def _counterfactual(root: Path, *, inferred: bool, contract_paths: bool) -> dict[str, Any]:
    result = json.loads(json.dumps(run_current(root)))
    plan, _diagnostics = parse_plan_markdown_with_diagnostics(
        (root / "plan.md").read_text(encoding="utf-8")
    )
    graph = task_graph_from_plan(plan)
    candidates, fallback, inventory_omissions = _candidate_inventory(root)
    knowledge_touches = {
        task_id: paths[:] for task_id, paths in result["declared_touches"].items()
    }
    task_provenance: dict[str, dict[str, set[str]]] = {
        task_id: {path: {"declared-scope"} for path in paths}
        for task_id, paths in knowledge_touches.items()
    }
    additions: list[dict[str, Any]] = inventory_omissions[:]
    if candidates is not None and (
        contract_paths or result["entry"] == "plan-ready"
    ):
        knowledge_touches, task_provenance, additions = _knowledge_touches(
            graph,
            result["declared_touches"],
            candidates,
            fallback,
            include_harvested=contract_paths,
        )
        additions = inventory_omissions + additions

    gotchas, _load_omissions, _available = _load_knowledge(root)
    inferred_scopes: dict[str, list[str]] = {}
    scope_provenance: dict[str, dict[str, str]] = {}
    if inferred and candidates is not None:
        inferred_scopes, scope_provenance, scope_omissions = _inferred_scope_candidates(
            root, candidates
        )
        additions.extend(scope_omissions)

    effective_scopes: dict[str, list[str]] = {}
    dropped_scope_count = 0
    dropped_scope_sources: set[str] = set()
    gotcha_by_id = {gotcha.id: gotcha for gotcha in gotchas}
    for gotcha_id, gotcha in gotcha_by_id.items():
        original_scope = sorted(set(gotcha.scope + inferred_scopes.get(gotcha_id, [])))
        bounded_scope = [
            scope
            for scope in original_scope
            if len(scope) <= MAX_CONTEXT_SCOPE_CHARS
        ][:MAX_CONTEXT_SCOPES_PER_GOTCHA]
        effective_scopes[gotcha_id] = bounded_scope
        dropped = len(original_scope) - len(bounded_scope)
        if dropped:
            dropped_scope_count += dropped
            dropped_scope_sources.add(gotcha.source_ref)
        if gotcha_id in scope_provenance:
            scope_provenance[gotcha_id] = {
                path: label
                for path, label in scope_provenance[gotcha_id].items()
                if path in bounded_scope
            }
    if dropped_scope_count:
        additions.append(
            _omission(
                "resolution_incomplete",
                dropped_scope_count,
                sorted(dropped_scope_sources)[:MAX_OMISSION_EXAMPLES],
            )
        )

    base_edges = {
        (edge["context_node_id"], edge["task_id"])
        for edge in result["knowledge_edges"]
    }
    task_ids = [node.id for node in graph.tasks]
    for context_node in result["context_nodes"]:
        gotcha_id = context_node["id"].removeprefix("context::knowledge::")
        if gotcha_id in inferred_scopes:
            context_node["scope"] = effective_scopes[gotcha_id]
            context_node["parameters"]["scope"] = ",".join(
                effective_scopes[gotcha_id]
            )
            context_node["parameters"]["knowledge_provenance"] = ",".join(
                f"{path}={label}"
                for path, label in sorted(scope_provenance[gotcha_id].items())
            )

    for gotcha_id, gotcha in gotcha_by_id.items():
        scope = effective_scopes[gotcha_id]
        context_id = f"context::knowledge::{gotcha_id}"
        linked = [
            task_id
            for task_id in task_ids
            if task_id in knowledge_touches
            and _scope_intersects(scope, knowledge_touches[task_id])
        ]
        fraction = len(linked) / len(task_ids) if task_ids else 0.0
        if len(task_ids) >= ANTI_HUB_MIN_TASKS and fraction >= ANTI_HUB_TASK_FRACTION:
            if any((context_id, task_id) not in base_edges for task_id in linked):
                lint = {
                    "context_node_id": context_id,
                    "linked_task_ids": linked,
                    "reason": "expanded knowledge coverage applies to a high fraction of tasks; base declared-scope edges were preserved and new inferred edges were withheld",
                }
                if lint not in result["hub_lints"]:
                    result["hub_lints"].append(lint)
            continue
        for task_id in linked:
            if (context_id, task_id) in base_edges:
                continue
            labels: set[str] = set()
            for scope_path in scope:
                for touch_path in knowledge_touches.get(task_id, []):
                    if not _scope_intersects([scope_path], [touch_path]):
                        continue
                    inferred_label = scope_provenance.get(gotcha_id, {}).get(
                        scope_path
                    )
                    if inferred_label is not None:
                        labels.add(inferred_label)
                    labels.update(
                        task_provenance.get(task_id, {}).get(touch_path, set())
                    )
            rationale = (
                "knowledge attachment matched " + ", ".join(sorted(labels))
                if labels
                else "knowledge attachment matched expanded scope"
            )
            result["knowledge_edges"].append(
                {
                    "task_id": task_id,
                    "context_node_id": context_id,
                    "rationale": rationale,
                }
            )
    result["knowledge_attachment_touches"] = {
        task_id: knowledge_touches[task_id] for task_id in sorted(knowledge_touches)
    }
    for omission in additions:
        if omission not in result["omissions"]:
            result["omissions"].append(omission)
    return result


def run_hv10(root: Path) -> dict[str, Any]:
    return _counterfactual(root, inferred=True, contract_paths=False)


def run_hv11(root: Path) -> dict[str, Any]:
    return _counterfactual(root, inferred=False, contract_paths=True)


def run_hv10_hv11(root: Path) -> dict[str, Any]:
    return _counterfactual(root, inferred=True, contract_paths=True)


def _tokens(value: str) -> list[str]:
    return [
        token
        for token in re.findall(r"[A-Za-z0-9_./-]+", _ascii_lower(value))
        if len(token) > 1
    ]


def bm25_shortlist(query: str, documents: list[tuple[str, str]]) -> list[tuple[str, float]]:
    """Return a deterministic stdlib BM25 shortlist, highest score first."""

    tokenized = [(key, _tokens(text)) for key, text in documents]
    if not tokenized:
        return []
    query_terms = Counter(_tokens(query))
    average_length = sum(len(tokens) for _, tokens in tokenized) / len(tokenized) or 1.0
    document_frequency = Counter(
        term for _, tokens in tokenized for term in set(tokens)
    )
    scored = []
    for key, tokens in tokenized:
        frequencies = Counter(tokens)
        score = 0.0
        for term, query_frequency in query_terms.items():
            frequency = frequencies[term]
            if not frequency:
                continue
            inverse = math.log(
                1.0
                + (len(tokenized) - document_frequency[term] + 0.5)
                / (document_frequency[term] + 0.5)
            )
            denominator = frequency + 1.2 * (
                1.0 - 0.75 + 0.75 * len(tokens) / average_length
            )
            score += query_frequency * inverse * frequency * 2.2 / denominator
        if score > 0.0:
            scored.append((key, score))
    return sorted(scored, key=lambda item: (-item[1], item[0]))[:8]


def _match_strength(scopes: list[str], touches: list[str]) -> int:
    strength = 0
    for scope in scopes:
        for touch in touches:
            if scope == touch:
                strength = max(strength, 3)
            elif touch.startswith(f"{scope}/") or scope.startswith(f"{touch}/"):
                strength = max(strength, 2)
            elif scope.rsplit("/", 1)[-1] == touch.rsplit("/", 1)[-1]:
                strength = max(strength, 1)
    return strength


def run_hv12(root: Path) -> dict[str, Any]:
    result = run_hv10_hv11(root)
    plan, _diagnostics = parse_plan_markdown_with_diagnostics(
        (root / "plan.md").read_text(encoding="utf-8")
    )
    graph = task_graph_from_plan(plan)
    gotchas, _omissions, _available = _load_knowledge(root)
    documents = [
        (gotcha.id, f"{gotcha.summary} {' '.join(gotcha.scope)}") for gotcha in gotchas
    ]
    gotcha_by_id = {gotcha.id: gotcha for gotcha in gotchas}
    candidates: dict[str, list[str]] = {}
    for task in graph.tasks:
        query = " ".join(
            [task.title] + task.inputs + task.outputs + task.acceptance
        )
        scored = bm25_shortlist(query, documents)
        ordered = sorted(
            scored,
            key=lambda item: (
                -_match_strength(
                    gotcha_by_id[item[0]].scope,
                    result["knowledge_attachment_touches"].get(task.id, []),
                ),
                -item[1],
                item[0],
            ),
        )[:3]
        for gotcha_id, _score in ordered:
            candidates.setdefault(gotcha_id, []).append(task.id)

    result["knowledge_edges"] = []
    result["hub_lints"] = []
    task_count = len(graph.tasks)
    for gotcha_id, linked in sorted(candidates.items()):
        context_id = f"context::knowledge::{gotcha_id}"
        fraction = len(linked) / task_count if task_count else 0.0
        if task_count >= ANTI_HUB_MIN_TASKS and fraction >= ANTI_HUB_TASK_FRACTION:
            result["hub_lints"].append(
                {
                    "context_node_id": context_id,
                    "linked_task_ids": linked,
                    "reason": "BM25 shortlist produced a high-fraction hub; attachment withheld",
                }
            )
            continue
        for task_id in linked:
            result["knowledge_edges"].append(
                {
                    "task_id": task_id,
                    "context_node_id": context_id,
                    "rationale": "bm25 shortlist ordered by path match strength",
                }
            )
    return result


RULESETS = {
    "current": run_current,
    "hv10": run_hv10,
    "hv11": run_hv11,
    "hv10+hv11": run_hv10_hv11,
    "hv12": run_hv12,
}


def run_ruleset(name: str, root: Path) -> dict[str, Any]:
    try:
        rule = RULESETS[name]
    except KeyError as error:
        raise ValueError(f"unknown retrieval rule set: {name}") from error
    return rule(root)
