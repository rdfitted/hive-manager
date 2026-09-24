#!/usr/bin/env python3
"""Read-only, aggregate-only baseline for worker knowledge placement.

Use --session DIR for session folders with prompts/, optionally paired with
--store-root ROOT for persisted state. Or give --store-root alone to scan its
session directories. Percentiles use the nearest-rank definition.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
from pathlib import Path


def _read_json(path: Path) -> dict | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def _summary(values: list[int]) -> dict:
    ordered = sorted(values)
    return {
        "count": len(ordered),
        "median": statistics.median(ordered) if ordered else None,
        "p90": ordered[math.ceil(0.9 * len(ordered)) - 1] if ordered else None,
    }


def discover_sessions(store_root: Path) -> list[Path]:
    """Find prompt folders from persisted project paths, with local fallback."""
    sessions = []
    for stored in sorted(path for path in store_root.iterdir() if path.is_dir()):
        metadata = _read_json(stored / "session.json") or {}
        project_path = metadata.get("project_path")
        if isinstance(project_path, str) and project_path:
            project_session = Path(project_path) / ".hive-manager" / stored.name
            if (project_session / "prompts").is_dir():
                sessions.append(project_session)
                continue
        sessions.append(stored)
    return sessions


def _graph_counts(graph: dict | None, task_id: object) -> tuple[int | None, int | None]:
    if graph is None or not isinstance(task_id, str) or not task_id:
        return None, None
    nodes = graph.get("nodes")
    edges = graph.get("edges")
    if not isinstance(nodes, list) or not isinstance(edges, list):
        return None, None
    global_ids = {
        node.get("id")
        for node in nodes
        if isinstance(node, dict)
        and isinstance(node.get("expansion"), dict)
        and isinstance(node["expansion"].get("parameters"), dict)
        and str(node["expansion"]["parameters"].get("source_ref", "")).startswith(
            "global:"
        )
    }
    attached = [
        edge
        for edge in edges
        if isinstance(edge, dict)
        and edge.get("target") == task_id
        and edge.get("kind") == "informs"
        and edge.get("provenance") == "knowledge"
    ]
    return len(attached), sum(edge.get("source") in global_ids for edge in attached)


def measure(sessions: list[Path], store_root: Path | None = None) -> dict:
    """Return only counts and distributions; never return pointers or contents."""
    metrics: dict[str, list[int]] = {
        "D1_attached_refs": [],
        "D2_global_refs": [],
        "D2_global_summary_included": [],
        "D3_injected_chars": [],
        "D3_kept_refs": [],
        "D3_dropped_refs": [],
        "D4_sampled": [],
        "D5_delivered_refs": [],
        "D6_role_and_task_refs": [],
        "D7_miss_sample_refs": [],
        "D8_tagged_refs": [],
    }
    spawns = 0
    unreadable_sidecars = 0
    for session in sessions:
        state_dir = (store_root / session.name if store_root else session) / "state"
        graph = _read_json(state_dir / "work-graph.json")
        for sidecar in sorted((session / "prompts").glob("*-context.json")):
            context = _read_json(sidecar)
            if context is None or context.get("schema_version") != "hive.spawn-context/v1":
                unreadable_sidecars += 1
                continue
            kept, dropped = context.get("kept"), context.get("dropped")
            if not isinstance(kept, list) or not isinstance(dropped, list):
                unreadable_sidecars += 1
                continue
            kept = [ref for ref in kept if isinstance(ref, dict)]
            dropped = [ref for ref in dropped if isinstance(ref, dict)]
            spawns += 1
            chars = sum(
                ref.get("chars", 0)
                for ref in kept
                if isinstance(ref.get("chars"), int) and ref["chars"] >= 0
            )
            metrics["D3_injected_chars"].append(chars)
            metrics["D3_kept_refs"].append(len(kept))
            metrics["D3_dropped_refs"].append(len(dropped))
            metrics["D4_sampled"].append(int(context.get("sampled") is True))
            metrics["D6_role_and_task_refs"].append(
                sum(ref.get("origin") == "role_and_task" for ref in kept)
            )
            miss = context.get("miss_sample_references", [])
            metrics["D7_miss_sample_refs"].append(
                len(miss) if isinstance(miss, list) else 0
            )
            metrics["D8_tagged_refs"].append(
                sum(isinstance(ref.get("tag"), str) for ref in kept)
            )
            task_id = context.get("plan_task_id")
            if isinstance(task_id, str) and task_id:
                metrics["D5_delivered_refs"].append(len(kept))
            attached, global_refs = _graph_counts(graph, task_id)
            if attached is not None:
                metrics["D1_attached_refs"].append(attached)
                metrics["D2_global_refs"].append(global_refs)
            global_summary = context.get("global_summary_included")
            if isinstance(global_summary, bool):
                metrics["D2_global_summary_included"].append(int(global_summary))
    return {
        "sessions": len(sessions),
        "spawns": spawns,
        "unreadable_sidecars": unreadable_sidecars,
        "metrics": {key: _summary(values) for key, values in metrics.items()},
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, action="append", default=[])
    parser.add_argument("--store-root", type=Path)
    args = parser.parse_args(argv)
    if not args.session and args.store_root is None:
        parser.error("give --session or --store-root")
    sessions = args.session or discover_sessions(args.store_root)
    print(json.dumps(measure(sessions, args.store_root), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
