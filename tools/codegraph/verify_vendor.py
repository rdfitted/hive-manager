#!/usr/bin/env python3
"""Verify vendored codegraph bytes and their upstream provenance metadata."""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
MANIFEST = ROOT / "vendor-manifest.json"
HEADER_HASH = re.compile(rb"source SHA-256 ([0-9a-fA-F]{64})")
SHA256 = re.compile(r"[0-9a-fA-F]{64}")
HASH_FIELDS = {"upstream_sha256", "normalized_sha256"}


def normalize_line_endings(content: bytes) -> bytes:
    """Return content with CRLF and lone CR line endings normalized to LF."""
    return content.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def load_manifest(path: Path) -> dict[str, dict[str, str]] | None:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        print(f"{path.name}: cannot read manifest: {error}", file=sys.stderr)
        return None
    except json.JSONDecodeError as error:
        print(
            f"{path.name}: invalid JSON at line {error.lineno}, column {error.colno}: "
            f"{error.msg}",
            file=sys.stderr,
        )
        return None

    if not isinstance(manifest, dict):
        print(f"{path.name}: invalid schema", file=sys.stderr)
        return None

    files = manifest.get("files")
    if manifest.get("version") != 2 or not isinstance(files, dict) or not files:
        print(f"{path.name}: invalid schema", file=sys.stderr)
        return None

    for name, hashes in files.items():
        relative = Path(name) if isinstance(name, str) else None
        if (
            relative is None
            or relative.is_absolute()
            or relative.name != name
            or name in (".", "..")
            or "/" in name
            or "\\" in name
        ):
            print(f"{path.name}: unsafe vendored path: {name!r}", file=sys.stderr)
            return None
        if not isinstance(hashes, dict) or set(hashes) != HASH_FIELDS:
            print(f"{path.name}: invalid hash fields for {name}", file=sys.stderr)
            return None
        if any(
            not isinstance(value, str) or SHA256.fullmatch(value) is None
            for value in hashes.values()
        ):
            print(f"{path.name}: invalid SHA-256 for {name}", file=sys.stderr)
            return None

    return files


def discover_vendored_files() -> set[str]:
    return {
        path.name
        for path in ROOT.glob("*.py")
        if HEADER_HASH.search(path.read_bytes()[:512])
    }


def main() -> int:
    files = load_manifest(MANIFEST)
    if files is None:
        return 1

    discovered = discover_vendored_files()
    declared = set(files)
    if discovered != declared:
        omitted = sorted(discovered - declared)
        stale = sorted(declared - discovered)
        if omitted:
            print(
                "vendor-manifest.json: unmanifested vendored files: " + ", ".join(omitted),
                file=sys.stderr,
            )
        if stale:
            print(
                "vendor-manifest.json: entries without vendored files: " + ", ".join(stale),
                file=sys.stderr,
            )
        return 1

    failures: list[str] = []
    for name, hashes in files.items():
        path = ROOT / name
        if not path.is_file():
            failures.append(f"{name}: missing")
            continue

        content = path.read_bytes()
        normalized_hash = hashlib.sha256(normalize_line_endings(content)).hexdigest()
        expected_normalized = hashes.get("normalized_sha256", "").lower()
        expected_upstream = hashes.get("upstream_sha256", "").lower()
        header_match = HEADER_HASH.search(content[:512])
        header_upstream = header_match.group(1).decode("ascii").lower() if header_match else ""

        file_failures = []
        if normalized_hash != expected_normalized:
            file_failures.append("normalized content differs")
        if header_upstream != expected_upstream:
            file_failures.append("header upstream hash differs from manifest")

        if file_failures:
            failures.append(f"{name}: {', '.join(file_failures)}")
            print(
                f"{name}: FAIL normalized={normalized_hash} "
                f"upstream={header_upstream or 'missing'}"
            )
        else:
            print(f"{name}: OK normalized={normalized_hash} upstream={header_upstream}")

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1

    print(f"Verified {len(files)} vendored files.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
