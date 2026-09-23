#!/usr/bin/env python3
# Vendored from ~/.claude/tools/judgment/calibration.py; source SHA-256 789139b9b6944ff57b3275ae42efb39606a7a8eb03f07854fbafd52b11a293b3
"""Calibration and interval math for judgment models — the canonical copy.

One implementation, reused everywhere. rdfitted/mainbrain_fitted#317 (wiki
rerank eval) and rdfitted/hive-manager#291 (hive QA) should VENDOR this file
with a provenance header and a hash check — the tools/codegraph/verify_vendor.py
pattern — rather than write a second ECE. Two ECEs that disagree in the third
decimal would make every cross-surface comparison meaningless.

Definitions (stated so a reader can check the numbers by hand):

  Expected Calibration Error, top-label, equal-width buckets:
      ECE = sum_b (n_b / N) * |mean_confidence_b - accuracy_b|
  where confidence is the probability the judge assigned to the label it
  picked, and accuracy is the share of those picks that matched ground truth.
  Bucket b covers [b/B, (b+1)/B); the last bucket is closed at 1.0.

  For a Noul (probability of yes, p): the pick is yes when p >= 0.5, and its
  confidence is max(p, 1 - p). A Noul has no separate `confidence` field; do
  not confuse the API's Choice/Score `confidence` (distribution concentration)
  with the top-label probability used here.

  Wilson score interval for a binomial proportion k/n at z (95% by default).
  Used wherever a rate is reported on small n — e.g. the DK2 miss rate, which
  at two samples per run stays wide for a long time and must say so.
"""

from __future__ import annotations

import math
from typing import Iterable, Sequence


def noul_to_pick(p: float, tau: float = 0.5) -> tuple[bool, float]:
    """(picked_yes, top_label_confidence) for a Noul probability."""
    yes = p >= tau
    return yes, (p if yes else 1.0 - p)


def ece(pairs: Iterable[tuple[float, bool]], n_buckets: int = 10) -> tuple[float | None, list[dict]]:
    """pairs: (top_label_confidence in [0,1], correct). Returns (ece, table).

    ece is None when there are no pairs — never 0.0, which would read as
    "perfectly calibrated" when the truth is "not measured".
    """
    pairs = [(min(max(float(c), 0.0), 1.0), bool(ok)) for c, ok in pairs]
    buckets: list[list[tuple[float, bool]]] = [[] for _ in range(n_buckets)]
    for c, ok in pairs:
        idx = min(int(c * n_buckets), n_buckets - 1)
        buckets[idx].append((c, ok))
    total = len(pairs)
    table = []
    err = 0.0
    for b, items in enumerate(buckets):
        lo, hi = b / n_buckets, (b + 1) / n_buckets
        n = len(items)
        if n:
            mean_c = sum(c for c, _ in items) / n
            acc = sum(1 for _, ok in items if ok) / n
            err += (n / total) * abs(mean_c - acc)
        else:
            mean_c = acc = None
        table.append({"bucket": f"{lo:.1f}-{hi:.1f}", "n": n, "mean_confidence": mean_c, "observed_accuracy": acc})
    return (err if total else None), table


def wilson(k: int, n: int, z: float = 1.96) -> tuple[float, float] | None:
    """95% Wilson interval for k successes in n trials; None when n == 0."""
    if n <= 0:
        return None
    phat = k / n
    denom = 1 + z * z / n
    centre = (phat + z * z / (2 * n)) / denom
    half = (z * math.sqrt(phat * (1 - phat) / n + z * z / (4 * n * n))) / denom
    return max(0.0, centre - half), min(1.0, centre + half)


def percentile(values: Sequence[float], q: float) -> float | None:
    """Nearest-rank percentile (q in [0,100]); None on empty input."""
    vals = sorted(v for v in values if v is not None)
    if not vals:
        return None
    rank = max(1, math.ceil(q / 100 * len(vals)))
    return vals[rank - 1]
