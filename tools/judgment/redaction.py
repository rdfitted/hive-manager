"""Repo-owned, fail-closed redaction preflight for judgment egress."""

from __future__ import annotations

import json
import math
import re
from pathlib import Path
from typing import Any


EMAIL = re.compile(r"(?<![\w.+-])[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}(?![\w.-])")


def load_dictionary(path: str | Path | None, profile_root: str | Path | None = None) -> list[Any]:
    """Load JSON entries or lines; profile directory slugs are optional additions."""
    entries = []
    if path:
        content = Path(path).read_text(encoding="utf-8")
        if not content.strip():
            raise ValueError("empty dictionary")
        if content.lstrip().startswith("["):
            entries = json.loads(content)
            if not isinstance(entries, list):
                raise ValueError("dictionary must be a list")
        elif content.lstrip().startswith("{"):
            raise ValueError("dictionary must be a list")
        else:
            entries = [line.strip() for line in content.splitlines() if line.strip()]
    if profile_root:
        root = Path(profile_root)
        if not root.is_dir():
            raise ValueError("unreadable profile root")
        for directory in root.iterdir():
            if directory.is_dir():
                entries.append(directory.name.replace("-", " ").replace("_", " "))
    if not entries:
        raise ValueError("empty dictionary")
    return entries


def _patterns(dictionary: Any) -> list[re.Pattern[str]]:
    if not isinstance(dictionary, (list, tuple)) or not dictionary:
        raise ValueError("missing dictionary")
    patterns = []
    for entry in dictionary:
        if isinstance(entry, str):
            term, kind = entry.strip(), "person"
            if term.lower().startswith("org:"):
                term, kind = term[4:].strip(), "organization"
        elif isinstance(entry, dict) and set(entry) == {"term", "kind"}:
            term, kind = entry["term"], entry["kind"]
        else:
            raise ValueError("invalid dictionary entry")
        if not isinstance(term, str) or not term.strip() or kind not in {"person", "organization"}:
            raise ValueError("invalid dictionary entry")
        term = term.strip()
        flags = re.IGNORECASE if kind == "organization" or any(
            len(part) >= 5 for part in re.findall(r"[A-Za-z]+", term)
        ) else 0
        patterns.append(re.compile(r"(?<!\w)" + re.escape(term) + r"(?!\w)", flags))
    return patterns


def redact(payload: Any, mode: str, dictionary: Any) -> tuple[Any | None, dict]:
    """Return the untouched safe payload or None; report classes, never matches."""
    report = {"hit_count": 0, "blocked": True, "reason_classes": []}
    if mode != "names":
        report["reason_classes"] = ["unknown-mode"]
        return None, report
    try:
        patterns = _patterns(dictionary)
    except (ValueError, TypeError, re.error):
        report["reason_classes"] = ["dictionary-unavailable"]
        return None, report
    try:
        strings = []

        def visit(value: Any) -> None:
            if isinstance(value, str):
                strings.append(value)
            elif isinstance(value, dict):
                for key, item in value.items():
                    if not isinstance(key, str):
                        raise ValueError("incomplete scan")
                    strings.append(key)
                    visit(item)
            elif isinstance(value, (list, tuple)):
                for item in value:
                    visit(item)
            elif isinstance(value, float) and not math.isfinite(value):
                raise ValueError("incomplete scan")
            elif value is not None and not isinstance(value, (bool, int, float)):
                raise ValueError("incomplete scan")

        visit(payload)
        classes = set()
        for value in strings:
            email_count = len(EMAIL.findall(value))
            if email_count:
                report["hit_count"] += email_count
                classes.add("email")
            for pattern in patterns:
                count = len(pattern.findall(value))
                if count:
                    report["hit_count"] += count
                    classes.add("dictionary")
        report["reason_classes"] = sorted(classes)
        if report["hit_count"]:
            return None, report
        report["blocked"] = False
        return payload, report
    except (ValueError, TypeError, OSError, UnicodeError, re.error, RecursionError):
        report["reason_classes"] = ["incomplete-scan"]
        return None, report
