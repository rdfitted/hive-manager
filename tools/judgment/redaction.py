"""Repo-owned, fail-closed redaction preflight for judgment egress."""

from __future__ import annotations

import json
import math
import re
from pathlib import Path
from typing import Any


EMAIL = re.compile(r"(?<![\w.+-])[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}(?![\w.-])")
# Separators are required: a bare ten-digit run in evidence is usually a timestamp.
PHONE = re.compile(r"(?<![\w.])(?:\+?\d{1,2}[\s.-])?(?:\(\d{3}\)\s?|\d{3}[\s.-])\d{3}[\s.-]\d{4}(?![\w])")
URL = re.compile(r"\bhttps?://(\[[^\]\s]+\]|[^\s/:\"'<>)\]]+)", re.IGNORECASE)
# Other hosts can identify a client by themselves. A path on these still gets the name scan.
SAFE_URL_HOSTS = re.compile(r"^(?:localhost|127\.0\.0\.1|\[::1\]|github\.com|(?:.+\.)?example\.(?:com|org|net)|.+\.test)$",
                            re.IGNORECASE)
PROFILE_FILES = ("CLIENT.md", "PARTNER.md", "PROFILE.md", "OPPORTUNITY.md")
# A contact table can key a row by role, and a role isn't a name.
ROLE_WORDS = {
    "finance", "accounting", "operations", "ops", "admin", "sales", "support", "team", "office", "unknown",
    "owner", "board", "staff", "management", "marketing", "estimating", "legal", "billing", "engineering",
    "field", "shipping", "orders", "returns", "customer", "customers", "vendor", "vendors", "contact",
}
NOT_DISTINCTIVE = {"the", "and", "good", "inc", "llc", "ltd", "corp", "group", "company", "client", "partners"}
# Bold first cells also head rows like "| **First team** |". Only these tables list people.
PERSON_TABLES = {"name", "person", "people", "contact", "candidate", "who"}


def _contact_names(text: str) -> list[str]:
    """Bold first cells of tables whose first column holds people."""
    names = []
    first_column = None
    previous_is_table = False
    for raw_line in text.splitlines():
        line = raw_line.strip()
        is_table = line.startswith("|")
        if is_table and not previous_is_table:
            first_column = line.split("|")[1].strip().strip("*").lower() if line.count("|") > 1 else None
        elif is_table and first_column in PERSON_TABLES:
            match = re.match(r"^\|\s*\*\*([^*|]{2,60})\*\*\s*\|", line)
            if match:
                names.append(match.group(1))
        previous_is_table = is_table
    return names


def _split_person(raw: str) -> list[str]:
    """A full name and its parts, including a quoted or bracketed nickname."""
    nicknames = re.findall(r'[("“]+\s*["“]?([^)"”]+?)["”]?\s*[)"”]', raw)
    base = re.sub(r"\s*\([^)]*\)\s*", " ", raw)
    base = " ".join(base.replace('"', "").replace("“", "").replace("”", "").split())
    parts = [part for part in re.split(r"[\s/]+", base) if len(part.strip(".")) >= 3 and part[0].isupper()]
    nicks = [word for nick in nicknames for word in nick.split() if len(word) >= 3 and word[0].isupper()]
    return [base, *parts, *nicks]


def _org_terms(name: str, *, committed: bool) -> list[str]:
    """A display name and its squashed form. A committed client's first word too.

    A prospect's first word is often an ordinary one ("Complete", "Action"), and
    blocking on it would discard most of a bench.
    """
    words = re.findall(r"[A-Za-z0-9]+", name)
    if not words:
        return []
    terms = [name.strip(), "".join(words)]
    first = next((word for word in words if word.lower() not in NOT_DISTINCTIVE), None)
    if committed and first and len(first) >= 3:
        terms.append(first)
    return terms


def memory_entries(root: str | Path) -> list[dict]:
    """Known names from a local memory root: clients/, partners/, pipeline/ and person memories.

    The names exist only at run time and only on this machine. Nothing here is fetched.
    """
    root = Path(root)
    orgs: set[str] = set()
    people: set[str] = set()
    for group in ("clients", "partners", "pipeline"):
        directory = root / group
        if not directory.is_dir():
            continue
        for sub in directory.iterdir():
            if not sub.is_dir() or sub.name.startswith("."):
                continue
            words = sub.name.replace("-", " ").replace("_", " ")
            tokens = [token for token in words.split() if len(token) >= 3 and token not in NOT_DISTINCTIVE]
            if group == "partners":
                people.add(words.title())
                people.update(token.title() for token in tokens)
            elif group == "clients":
                orgs.update({sub.name, words, *tokens, *(f"{token}brain" for token in tokens)})
            else:
                orgs.update({sub.name, words})
            for profile in (sub / name for name in PROFILE_FILES):
                if not profile.is_file():
                    continue
                text = profile.read_text(encoding="utf-8", errors="replace")
                for name in _contact_names(text):
                    people.update(_split_person(name))
                match = re.search(r"^- \*\*Name:\*\*\s*(.+)$", text, re.M)
                if match:
                    committed = group == "clients"
                    orgs.update(_org_terms(re.sub(r"\s*\(.*$", "", match.group(1)), committed=committed))
                    for quoted in re.findall(r'"([^"]{2,40})"', match.group(1)):
                        orgs.update(_org_terms(quoted, committed=committed))
    for index, committed in ((root / "clients" / "CLIENTS.md", True), (root / "pipeline" / "PIPELINE.md", False)):
        if index.is_file():
            for name in re.findall(r"\*\*\[([^\]]{2,60})\]\(", index.read_text(encoding="utf-8", errors="replace")):
                orgs.update(_org_terms(name, committed=committed))
    for person in (root / "projects").glob("*/memory/person_*.md"):
        name = person.stem.removeprefix("person_").replace("_", " ").title()
        people.update([name, *(part for part in name.split() if len(part) >= 3)])
    people = {name for name in people if len(name) >= 3
              and name.lower() not in ROLE_WORDS and name.lower() not in NOT_DISTINCTIVE}
    return ([{"term": term, "kind": "organization"} for term in sorted(term for term in orgs if len(term) >= 2)]
            + [{"term": term, "kind": "person"} for term in sorted(people)])


def load_dictionary(path: str | Path | None, profile_root: str | Path | None = None,
                    memory_root: str | Path | None = None) -> list[Any]:
    """Load JSON entries or lines, plus profile directory slugs and memory names if given."""
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
            if directory.is_dir() and not directory.name.startswith("."):
                entries.append(directory.name.replace("-", " ").replace("_", " "))
    if memory_root:
        if not Path(memory_root).is_dir():
            raise ValueError("unreadable memory root")
        entries.extend(memory_entries(memory_root))
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
        # Only a letter continues a word, so "name_orders" and "name2" still match.
        patterns.append(re.compile(r"(?<![^\W\d_])" + re.escape(term) + r"(?![^\W\d_])", flags))
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
            phone_count = len(PHONE.findall(value))
            if phone_count:
                report["hit_count"] += phone_count
                classes.add("phone")
            url_count = sum(not SAFE_URL_HOSTS.match(host) for host in URL.findall(value))
            if url_count:
                report["hit_count"] += url_count
                classes.add("url")
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
