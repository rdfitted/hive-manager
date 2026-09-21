#!/usr/bin/env python3
"""Offline retrieval replay entry points.

T4 exposes the deterministic rule-set matrix here; T5 adds saved-session
walking, downstream diffs, aggregate reporting, and explicit ledger plumbing.
"""

from __future__ import annotations

from pathlib import Path
from typing import Iterable

from retrieval_rules import run_ruleset


RULESET_ORDER = ("current", "hv10", "hv11", "hv10+hv11", "hv12")


def evaluate_fixture(
    root: Path, rulesets: Iterable[str] = RULESET_ORDER
) -> dict[str, dict]:
    """Evaluate one materialized project fixture in stable rule-set order."""

    return {ruleset: run_ruleset(ruleset, root) for ruleset in rulesets}
