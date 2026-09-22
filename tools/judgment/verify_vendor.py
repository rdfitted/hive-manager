#!/usr/bin/env python3
"""Fail-closed verification for the vendored judgment-ledger files."""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
MANIFEST_NAME = "vendor-manifest.json"
HEADER_HASH = re.compile(rb"source SHA-256 ([0-9a-fA-F]{64})")
SHA256 = re.compile(r"[0-9a-f]{64}")
FILE_FIELDS = {"source", "upstream_sha256", "normalized_sha256"}


def normalize_line_endings(content: bytes) -> bytes:
    return content.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def load_manifest(path: Path) -> dict[str, dict[str, str]]:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path.name}: cannot load manifest: {error}") from error
    if not isinstance(manifest, dict) or manifest.get("version") != 1:
        raise ValueError(f"{path.name}: invalid schema")
    files = manifest.get("files")
    if not isinstance(files, dict) or not files:
        raise ValueError(f"{path.name}: invalid files map")
    for name, metadata in files.items():
        relative = Path(name) if isinstance(name, str) else None
        if (
            relative is None
            or relative.is_absolute()
            or relative.name != name
            or name in {".", ".."}
            or "/" in name
            or "\\" in name
        ):
            raise ValueError(f"{path.name}: unsafe vendored path: {name!r}")
        if not isinstance(metadata, dict) or set(metadata) != FILE_FIELDS:
            raise ValueError(f"{path.name}: invalid metadata for {name}")
        source = metadata.get("source")
        if not isinstance(source, str) or not source:
            raise ValueError(f"{path.name}: missing source for {name}")
        for field in ("upstream_sha256", "normalized_sha256"):
            value = metadata.get(field)
            if not isinstance(value, str) or SHA256.fullmatch(value) is None:
                raise ValueError(f"{path.name}: invalid {field} for {name}")
    return files


def discover_vendored_files(root: Path) -> set[str]:
    discovered: set[str] = set()
    for path in root.iterdir():
        if not path.is_file():
            continue
        if path.suffix == ".py" and HEADER_HASH.search(path.read_bytes()[:512]):
            discovered.add(path.name)
            continue
        if path.suffix == ".json":
            try:
                value: Any = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if isinstance(value, dict) and value.get("$id") == "judgment-ledger/v1":
                discovered.add(path.name)
    return discovered


def verify(root: Path = ROOT) -> list[str]:
    files = load_manifest(root / MANIFEST_NAME)
    declared = set(files)
    discovered = discover_vendored_files(root)
    if discovered != declared:
        omitted = sorted(discovered - declared)
        stale = sorted(declared - discovered)
        problems = []
        if omitted:
            problems.append("unmanifested vendored files: " + ", ".join(omitted))
        if stale:
            problems.append("manifest entries without vendored files: " + ", ".join(stale))
        raise ValueError("; ".join(problems))

    results: list[str] = []
    for name in sorted(files):
        path = root / name
        content = path.read_bytes()
        normalized_hash = hashlib.sha256(normalize_line_endings(content)).hexdigest()
        metadata = files[name]
        if normalized_hash != metadata["normalized_sha256"]:
            raise ValueError(f"{name}: normalized content differs")
        if path.suffix == ".py":
            header = HEADER_HASH.search(content[:512])
            header_hash = header.group(1).decode("ascii").lower() if header else ""
            if header_hash != metadata["upstream_sha256"]:
                raise ValueError(f"{name}: header upstream hash differs from manifest")
        results.append(f"{name}: OK")
    return results


def main() -> int:
    try:
        results = verify()
    except (OSError, ValueError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    for result in results:
        print(result)
    print("OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
