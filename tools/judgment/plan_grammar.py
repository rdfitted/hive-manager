"""A dependency-free mirror of Hive Manager's markdown plan grammar.

The parser intentionally preserves the small quirks of
``actions/coordination.rs``.  It is used by the retrieval replay tooling, so
"helpful" Markdown parsing would make historical replay disagree with Rust.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional


def _ascii_lower(value: str) -> str:
    return value.translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"))


def rust_lines(value: str) -> list[str]:
    """Return the equivalent of Rust ``str::lines`` for text fixtures."""

    if not value:
        return []
    parts = value.split("\n")
    if parts and parts[-1] == "":
        parts.pop()
    for index in range(len(parts) - (0 if value.endswith("\n") else 1)):
        if parts[index].endswith("\r"):
            parts[index] = parts[index][:-1]
    return parts


@dataclass(eq=True)
class PlanTask:
    id: str
    title: str
    description: str = ""
    status: str = "pending"
    assignee: Optional[str] = None
    assignee_label: Optional[str] = None
    priority: Optional[str] = None
    tier: str = "medium"
    depends_on: list[str] = field(default_factory=list)
    inputs: list[str] = field(default_factory=list)
    outputs: list[str] = field(default_factory=list)
    acceptance: list[str] = field(default_factory=list)
    explicit_id: bool = False
    checkbox_source: bool = False
    assignee_recognized: bool = True
    tier_recognized: bool = True
    tier_source: Optional[str] = None


@dataclass(eq=True)
class SessionPlan:
    title: str
    summary: str
    tasks: list[PlanTask]
    raw_content: str


def parse_plan_markdown_with_diagnostics(content: str) -> tuple[SessionPlan, list[str]]:
    title = ""
    summary = ""
    tasks: list[PlanTask] = []
    diagnostics: list[str] = []
    current_section = ""
    task_counter = 0

    for line_index, line in enumerate(rust_lines(content)):
        trimmed = line.strip()
        if trimmed.startswith("# ") and not title:
            title = trimmed[2:].strip()
            continue

        if trimmed.startswith("## "):
            section_name = _ascii_lower(trimmed[3:].strip())
            if "summary" in section_name or "overview" in section_name:
                current_section = "summary"
            elif "task" in section_name or "plan" in section_name:
                current_section = "tasks"
            else:
                current_section = ""
            continue

        if current_section == "summary" and trimmed and not trimmed.startswith("#"):
            summary = f"{summary} {trimmed}" if summary else trimmed
            continue

        if current_section == "tasks":
            task, error, task_counter = _parse_task_line(trimmed, task_counter)
            if error is not None:
                diagnostics.append(f"line {line_index + 1}: {error}")
            if task is not None:
                tasks.append(task)

    if not title:
        title = "Plan in Progress..."
    if any(task.explicit_id for task in tasks):
        diagnostics.extend(
            f"schedulable checkbox task is missing a stable T<number>: id: {task.title}"
            for task in tasks
            if task.checkbox_source and not task.explicit_id
        )
    return SessionPlan(title, summary, tasks, content), diagnostics


def parse_plan_markdown(content: str) -> SessionPlan:
    return parse_plan_markdown_with_diagnostics(content)[0]


def _parse_task_line(
    line: str, counter: int
) -> tuple[Optional[PlanTask], Optional[str], int]:
    trimmed = line.strip()
    checkbox_prefixes = ("- [ ]", "* [ ]", "- [x]", "* [x]", "- [X]", "* [X]")
    checkbox_source = any(trimmed.startswith(prefix) for prefix in checkbox_prefixes)
    if not trimmed or trimmed.startswith("#"):
        return None, None, counter

    if trimmed.startswith(("- [ ]", "* [ ]")):
        status, rest = "pending", trimmed[5:].strip()
    elif trimmed.startswith(("- [x]", "* [x]", "- [X]", "* [X]")):
        status, rest = "completed", trimmed[5:].strip()
    elif trimmed.startswith(("- ", "* ")):
        status, rest = "pending", trimmed[2:].strip()
    elif trimmed[0].isascii() and trimmed[0].isdigit():
        position = trimmed.find(". ")
        if position < 0:
            return None, None, counter
        status, rest = "pending", trimmed[position + 2 :].strip()
    else:
        return None, None, counter
    if not rest:
        return None, None, counter

    counter += 1
    cleaned, metadata, metadata_error = _extract_task_metadata(rest)
    cleaned, priority = _extract_priority(cleaned)
    cleaned, explicit_id = _extract_explicit_task_id(cleaned)
    cleaned, assignee, assignee_label, assignee_recognized = _extract_assignee(cleaned)
    task = PlanTask(
        id=explicit_id or f"task-{counter}",
        title=cleaned.strip(),
        status=status,
        assignee=assignee,
        assignee_label=assignee_label,
        priority=priority,
        tier=metadata["tier"],
        depends_on=metadata["depends_on"],
        inputs=metadata["inputs"],
        outputs=metadata["outputs"],
        acceptance=metadata["acceptance"],
        explicit_id=explicit_id is not None,
        checkbox_source=checkbox_source,
        assignee_recognized=assignee_recognized,
        tier_recognized=metadata["tier_recognized"],
        tier_source=metadata["tier_source"],
    )
    return task, metadata_error, counter


def _default_metadata() -> dict[str, object]:
    return {
        "depends_on": [],
        "inputs": [],
        "outputs": [],
        "acceptance": [],
        "tier": "medium",
        "tier_recognized": True,
        "tier_source": None,
    }


def _extract_task_metadata(text: str) -> tuple[str, dict[str, object], Optional[str]]:
    cleaned = text
    metadata = _default_metadata()
    destinations = {"deps": "depends_on", "inputs": "inputs", "outputs": "outputs", "acceptance": "acceptance"}
    for key in ("deps", "inputs", "outputs", "acceptance", "tier"):
        marker = f"({key}:"
        while True:
            haystack = _ascii_lower(cleaned) if key == "tier" else cleaned
            start = haystack.find(marker)
            if start < 0:
                break
            value_start = start + len(marker)
            close = cleaned.find(")", value_start)
            if close < 0:
                return text, _default_metadata(), f"unterminated ({key}: ...) metadata"
            raw_value = cleaned[value_start:close].strip()
            values = [value.strip() for value in cleaned[value_start:close].split(",") if value.strip()]
            if key != "tier":
                target = metadata[destinations[key]]
                assert isinstance(target, list)
                for value in values:
                    if value not in target:
                        target.append(value)
            else:
                lowered = _ascii_lower(raw_value)
                recognized = lowered in {"low", "medium", "high", "critical"}
                if recognized:
                    if metadata["tier_recognized"]:
                        metadata["tier"] = lowered
                        metadata["tier_source"] = raw_value
                else:
                    if metadata["tier_recognized"]:
                        metadata["tier_source"] = raw_value
                    metadata["tier"] = "medium"
                    metadata["tier_recognized"] = False
            cleaned = cleaned[:start] + cleaned[close + 1 :]
    return cleaned, metadata, None


def _extract_priority(text: str) -> tuple[str, Optional[str]]:
    priorities = (
        ("[HIGH]", "high"), ("[P1]", "high"), ("[CRITICAL]", "high"),
        ("[MEDIUM]", "medium"), ("[P2]", "medium"), ("[MED]", "medium"),
        ("[LOW]", "low"), ("[P3]", "low"),
    )
    tokens = text.split()
    for marker, priority in priorities:
        if any(_ascii_lower(token) == _ascii_lower(marker) for token in tokens):
            return " ".join(token for token in tokens if _ascii_lower(token) != _ascii_lower(marker)), priority
    return text, None


def _extract_explicit_task_id(text: str) -> tuple[str, Optional[str]]:
    if ":" not in text:
        return text, None
    candidate, remainder = text.split(":", 1)
    candidate = candidate.strip()
    if candidate.startswith("[") and "]" in candidate[1:]:
        candidate = candidate.split("]", 1)[1].lstrip()
    explicit = len(candidate) > 1 and candidate[0] in "Tt" and all(
        character.isascii() and character.isdigit() for character in candidate[1:]
    )
    return (remainder.lstrip(), candidate) if explicit else (text, None)


def _extract_assignee(text: str) -> tuple[str, Optional[str], Optional[str], bool]:
    for separator in ("->", "→"):
        if separator not in text:
            continue
        title, assignee = text.split(separator, 1)
        assignee = assignee.strip()
        if not assignee:
            return title, None, None, False
        first_token = assignee.split()[0]
        principal = _normalize_principal_token(first_token)
        if principal is not None:
            label = assignee[len(first_token) :].strip()
            return title, principal, label or None, True
        return title, assignee, None, False
    return text, None, None, True


def _normalize_principal_token(token: str) -> Optional[str]:
    if len(token) > 1 and token[0] in "Pp" and all(
        character.isascii() and character.isdigit() for character in token[1:]
    ):
        return f"P{token[1:]}"
    if _ascii_lower(token) == "queen":
        return "Queen"
    if _ascii_lower(token) == "operator":
        return "Operator"
    lowered = _ascii_lower(token)
    if lowered.startswith("worker-") and lowered[7:] and all(
        character.isascii() and character.isdigit() for character in lowered[7:]
    ):
        return f"worker-{lowered[7:]}"
    return None
