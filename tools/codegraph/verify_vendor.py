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
EXPECTED_FILES = {"codegraph.py", "codegraph_rs.py", "codegraph_ts.py"}


def main() -> int:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    files = manifest.get("files")
    if (
        manifest.get("version") != 1
        or not isinstance(files, dict)
        or set(files) != EXPECTED_FILES
        or any(not isinstance(hashes, dict) for hashes in files.values())
    ):
        print("vendor-manifest.json: invalid schema", file=sys.stderr)
        return 1

    failures: list[str] = []
    for name, hashes in files.items():
        path = ROOT / name
        if not path.is_file():
            failures.append(f"{name}: missing")
            continue

        content = path.read_bytes()
        vendored_hash = hashlib.sha256(content).hexdigest()
        expected_vendored = hashes.get("vendored_sha256", "").lower()
        expected_upstream = hashes.get("upstream_sha256", "").lower()
        header_match = HEADER_HASH.search(content[:512])
        header_upstream = header_match.group(1).decode("ascii").lower() if header_match else ""

        file_failures = []
        if vendored_hash != expected_vendored:
            file_failures.append("vendored bytes differ")
        if header_upstream != expected_upstream:
            file_failures.append("header upstream hash differs from manifest")

        if file_failures:
            failures.append(f"{name}: {', '.join(file_failures)}")
            print(
                f"{name}: FAIL vendored={vendored_hash} upstream={header_upstream or 'missing'}"
            )
        else:
            print(f"{name}: OK vendored={vendored_hash} upstream={header_upstream}")

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1

    print(f"Verified {len(files)} vendored files.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
