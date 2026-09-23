#!/usr/bin/env python3
# Vendored from ~/.claude/tools/judgment/audit.py; source SHA-256 2235b39a10c0068e237222fd4e93f8dd56a01ff111f25208d4ee6bd36f1bf24b
"""The scorecard — success rates for every judgment surface, one report.

Reads every judgment-ledger/v1 stream (docket, hive, and the wiki retrieval
ledger through an adapter), joins decisions to outcomes, and reports per
(surface, judge): agreement with ground truth on a fixed held-out half,
calibration, repeatability, fail-open rate, latency and cost. For dockets it
adds action rate, abandonment, proposal agreement and the audit-sample miss
rate with a Wilson interval.

It PROPOSES promotion and demotion. It never promotes. `--enforce` applies
demotions only, because demotion moves a surface toward the human and is the
one direction safe to automate. Promotion is Ryan's call on this report.

Honesty rules baked in:
  * Rows with unknown sampling (null) make repeatability NOT COMPARABLE —
    never a pass. The LangChain Jev-as-judge study that prompted this work
    compared a deterministic judge against LLMs at provider-default
    temperature; this report refuses to make that comparison silently.
  * The held-out split is a pure function of the judged item's hash. It is
    printed, never re-rolled, and every agreement number says which half.
  * Small n is flagged. A rate on 7 samples is printed with its interval.
  * Unjoined decisions and malformed rows are counted, never dropped quietly.

Tracking: rdfitted/Claude-Code-Setup#25 (JL2). Epic #23.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import calibration as C  # noqa: E402
import ledger as L  # noqa: E402

AUTHORITY = {"human-label": 5, "operator-override": 4, "user-answer": 3, "model-ack": 2, "downstream": 1}
INCUMBENTS = ("code", "incumbent-llm")
NEXT_MODE = {"shadow": "advisory", "advisory": "gating"}
PREV_MODE = {"gating": "advisory", "advisory": "shadow"}


# ---------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------

def adapt_retrievals(path: Path) -> tuple[list[dict], dict]:
    """Synthesize decision rows from the wiki retrieval ledger (mainbrain #313).

    Only hits carrying `p_rel` (written once #313's rerank lands) become rows;
    that epic's ledger shape is unchanged. Returns (rows, summary).
    """
    rows: list[dict] = []
    summary = {"records": 0, "records_with_typed_hits": 0, "typed_hits": 0}
    if not path or not Path(path).exists():
        return rows, summary
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            summary["records"] += 1
            typed = 0
            for j, hit in enumerate(rec.get("hits") or []):
                p = hit.get("p_rel")
                if p is None:
                    continue
                typed += 1
                state = {"keywords": rec.get("keywords"), "source": hit.get("source"),
                         "page": hit.get("page"), "excerpt": hit.get("excerpt")}
                rows.append({
                    "v": 1, "kind": "decision", "ts": rec.get("ts"),
                    "decision_id": f"retrieval:{i}:{j}",
                    "surface": "wiki.retrieval.relevance",
                    "subject_ref": {"line": i, "hit": j, "page": hit.get("page")},
                    "state_hash": L.sha256_of(state), "state_ref": None,
                    "question_id": "wiki.retrieval.relevance", "question_version": rec.get("rerank_question_version"),
                    "judge": "jev", "model": rec.get("rerank_model"), "sampling": {},
                    "mode": "shadow", "answer": p, "probabilities": None, "confidence": None,
                    "routed": "none", "latency_ms": rec.get("rerank_ms"), "cost_usd": None, "error": None,
                })
            if typed:
                summary["records_with_typed_hits"] += 1
                summary["typed_hits"] += typed
    return rows, summary


def load(paths: list[Path] | None, retrievals: Path | None) -> tuple[dict, dict, dict]:
    stats: dict = {"malformed": 0}
    decisions: dict[str, dict] = {}
    outcomes: dict[str, list[dict]] = defaultdict(list)
    for r in L.read_records(paths, stats):
        if r.get("kind") == "decision" and r.get("decision_id"):
            decisions[r["decision_id"]] = r
        elif r.get("kind") == "outcome" and r.get("decision_id"):
            outcomes[r["decision_id"]].append(r)
    rrows, rsum = adapt_retrievals(retrievals) if retrievals else ([], {})
    for r in rrows:
        decisions[r["decision_id"]] = r
    stats["retrievals"] = rsum
    return decisions, outcomes, stats


# ---------------------------------------------------------------------------
# Joins and predictions
# ---------------------------------------------------------------------------

def docket_name(surface: str) -> str | None:
    parts = surface.split(".", 2)
    if len(parts) == 3 and parts[0] == "docket":
        return parts[2]
    return None


def truth_outcome(row: dict, decisions: dict, outcomes: dict) -> dict | None:
    """Highest-authority, then latest, outcome for a row — its own, else its
    source decision's (shadow rows inherit the incumbent's ground truth)."""
    cands = list(outcomes.get(row["decision_id"], []))
    src = row.get("source_decision_id")
    if not cands and src:
        cands = list(outcomes.get(src, []))
    if not cands:
        return None
    return max(cands, key=lambda o: (AUTHORITY.get(o.get("source"), 0), o.get("ts", "")))


def item_hash(row: dict, decisions: dict) -> str:
    """Identity of the judged ITEM, shared by an incumbent row and every shadow
    row that re-judges it — so the held-out split and the incumbent comparison
    line up across judges even when they were sent different state."""
    src = row.get("source_decision_id")
    if src and src in decisions and decisions[src].get("state_hash"):
        return decisions[src]["state_hash"]
    return row.get("state_hash") or L.sha256_of(row["decision_id"])


def is_heldout(h: str) -> bool:
    """Deterministic split on the item hash's last hex digit. Never re-rolled."""
    try:
        return int(h[-1], 16) & 1 == 1
    except (ValueError, IndexError):
        return False


def question_type(surface: str, tables: dict) -> str:
    qt = tables.get("surfaces", {}).get(surface, {}).get("question_type")
    if qt:
        return qt
    if surface.startswith("docket.admission."):
        return "binary"
    if surface.startswith("docket.disposition."):
        return "multiclass"
    return "scored" if ".scored" in surface else "multiclass"


def truth_value(surface: str, label: Any, tables: dict) -> Any:
    """Ground truth in the prediction's space. None = excluded (undecided)."""
    if label is None or label == "undecided":
        return None
    if surface.startswith("wiki.retrieval."):
        return {"used": True, "unused": False}.get(str(label))
    if surface.startswith("docket.admission."):
        cfg = tables.get("docket", {}).get("surfaces", {}).get(docket_name(surface) or "", {})
        return label not in set(cfg.get("leave_alone", []))
    return label


def prediction(row: dict, qtype: str, tau: float) -> tuple[Any, float | None]:
    """(predicted value, top-label confidence for ECE or None)."""
    a = row.get("answer")
    if a is None:
        return None, None
    if qtype == "binary":
        if isinstance(a, bool):
            return a, None
        if isinstance(a, (int, float)):
            yes, conf = C.noul_to_pick(float(a), 0.5)
            return float(a) >= tau, conf
        return str(a) == "admitted", None
    if qtype == "scored":
        probs = row.get("probabilities") or {}
        conf = None
        if probs:
            top = max(probs.items(), key=lambda kv: kv[1])
            conf = float(top[1])
        return a, conf
    probs = row.get("probabilities") or {}
    return a, (float(probs[a]) if isinstance(a, str) and a in probs else None)


def agrees(pred: Any, truth: Any, qtype: str) -> bool:
    if qtype == "scored":
        try:
            return abs(float(pred) - float(truth)) <= 1.0
        except (TypeError, ValueError):
            return pred == truth
    return pred == truth


def repeat_key(row: dict) -> str:
    return L.canonical_json([row.get("surface"), row.get("question_version"), row.get("state_hash"),
                             row.get("judge"), row.get("model"), row.get("sampling")])


def decision_of(row: dict, qtype: str, tau: float) -> Any:
    """What the system would actually DO with an answer: the thresholded Noul,
    the top Choice label, the nearest Score level.

    Repeatability is judged on this, not on raw probabilities. Measured
    2026-09-21 against jev-1.13.0: identical state, three calls, needs_human
    p = 0.88 / 0.86 / 0.87 — the decision held, the probability jittered by
    ~0.01. Exact-probability matching would flag every replay as a violation
    and demote the judge on a measurement artifact. Jitter is reported
    separately; a jitter wide enough to straddle tau shows up here as a real
    violation, because the decision itself flips.
    """
    pred, _ = prediction(row, qtype, tau)
    if qtype == "scored" and isinstance(pred, (int, float)) and not isinstance(pred, bool):
        return round(float(pred))
    return pred


def spread(group: list[dict]) -> float:
    """Largest max-min spread of any probability (or numeric answer) in a group."""
    by_label: dict[str, list[float]] = defaultdict(list)
    for r in group:
        probs = r.get("probabilities")
        if isinstance(probs, dict) and probs:
            for k, v in probs.items():
                if isinstance(v, (int, float)):
                    by_label[str(k)].append(float(v))
        elif isinstance(r.get("answer"), (int, float)) and not isinstance(r.get("answer"), bool):
            by_label["_answer"].append(float(r["answer"]))
    return max((max(v) - min(v) for v in by_label.values() if v), default=0.0)


# ---------------------------------------------------------------------------
# Scoring
# ---------------------------------------------------------------------------

def score_group(rows: list[dict], decisions: dict, outcomes: dict, tables: dict,
                now: datetime) -> dict:
    surface, judge = rows[0]["surface"], rows[0]["judge"]
    qtype = question_type(surface, tables)
    scfg = tables.get("surfaces", {}).get(surface, {})
    tau = scfg.get("tau")
    tau = 0.5 if tau is None else float(tau)

    # Repeatability — every group of identical (question, state, judge, model, sampling).
    groups: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        if r.get("error") is None and r.get("answer") is not None and r.get("state_hash"):
            groups[repeat_key(r)].append(r)
    multi = {k: g for k, g in groups.items() if len(g) > 1}
    unknown_sampling = any(r.get("sampling") is None for r in rows)
    violations = []
    spreads = []
    for k, g in multi.items():
        spreads.append(spread(g))
        made = {L.canonical_json(decision_of(r, qtype, tau)) for r in g}
        if len(made) > 1:
            violations.append({"state_hash": g[0].get("state_hash"), "n": len(g),
                               "answers": sorted(made),
                               "raw_answers": sorted({L.canonical_json(r.get("answer")) for r in g}),
                               "decision_ids": [r["decision_id"] for r in g[:5]]})

    # One prediction per judged item (replays collapse to their first answer;
    # a replay that disagrees is already a repeatability violation above).
    per_item: dict[str, dict] = {}
    for r in sorted(rows, key=lambda x: x.get("ts", "")):
        if r.get("error") is not None or r.get("answer") is None:
            continue
        per_item.setdefault(item_hash(r, decisions), r)

    joined, unjoined = [], 0
    for h, r in per_item.items():
        o = truth_outcome(r, decisions, outcomes)
        if o is None:
            unjoined += 1
            continue
        t = truth_value(surface, o.get("label"), tables)
        if t is None:
            continue
        pred, conf = prediction(r, qtype, tau)
        joined.append({"hash": h, "ts": r.get("ts", ""), "pred": pred, "truth": t, "conf": conf,
                       "ok": agrees(pred, t, qtype), "heldout": is_heldout(h)})

    held = [j for j in joined if j["heldout"]]
    tune = [j for j in joined if not j["heldout"]]

    def rate(xs):
        return (sum(1 for x in xs if x["ok"]) / len(xs)) if xs else None

    mae = None
    if qtype == "scored" and held:
        try:
            mae = sum(abs(float(x["pred"]) - float(x["truth"])) for x in held) / len(held)
        except (TypeError, ValueError):
            mae = None
    ece_val, ece_table = C.ece([(x["conf"], x["ok"]) for x in held if x["conf"] is not None])

    errors = [r for r in rows if r.get("error")]
    week = now - timedelta(days=7)
    recent = [r for r in rows if (_ts(r) or now) >= week]
    recent_err = [r for r in recent if r.get("error")]
    lat = [r.get("latency_ms") for r in rows if isinstance(r.get("latency_ms"), (int, float))]
    cost = [r.get("cost_usd") for r in rows if isinstance(r.get("cost_usd"), (int, float))]
    window = int(tables.get("demotion", {}).get("rolling_window", 50))
    rolling = sorted(joined, key=lambda x: x["ts"])[-window:]

    return {
        "surface": surface, "judge": judge, "question_type": qtype,
        "mode": scfg.get("mode") if judge not in INCUMBENTS else "incumbent",
        "rows": len(rows), "items": len(per_item),
        "question_versions": len({r.get("question_version") for r in rows}),
        "models": sorted({str(r.get("model")) for r in rows if r.get("model")}),
        "joined": len(joined), "unjoined": unjoined,
        "n_heldout": len(held), "n_tune": len(tune),
        "agreement_heldout": rate(held), "agreement_tune": rate(tune), "mae_heldout": mae,
        "heldout_hashes": [x["hash"] for x in held],
        "heldout_ok": {x["hash"]: x["ok"] for x in held},
        "ece_heldout": ece_val, "reliability": ece_table if ece_val is not None else None,
        "repeat_groups": len(multi), "repeat_violations": violations,
        "repeat_comparable": not unknown_sampling,
        "jitter_max": max(spreads) if spreads else None,
        "jitter_p95": C.percentile(spreads, 95),
        "jitter_groups": sum(1 for s in spreads if s > 0),
        "fail_open_rate": (len(errors) / len(rows)) if rows else None,
        "fail_open_rate_7d": (len(recent_err) / len(recent)) if recent else None,
        "latency_p50": C.percentile(lat, 50), "latency_p95": C.percentile(lat, 95),
        "cost_total": sum(cost) if cost else None,
        "rolling_agreement": rate(rolling), "rolling_n": len(rolling),
    }


def _ts(r: dict) -> datetime | None:
    try:
        return datetime.fromisoformat(str(r.get("ts", "")).replace("Z", "+00:00"))
    except ValueError:
        return None


def docket_metrics(decisions: dict, outcomes: dict, tables: dict, ledger_dir: Path | None) -> list[dict]:
    small = int(tables.get("targets", {}).get("small_n_warning", 100))
    by_surface: dict[str, list[dict]] = defaultdict(list)
    for r in decisions.values():
        if r.get("surface", "").startswith("docket.admission.") and r.get("judge") == "code":
            by_surface[r["surface"]].append(r)
    out = []
    for surface, rows in sorted(by_surface.items()):
        name = docket_name(surface)
        cfg = tables.get("docket", {}).get("surfaces", {}).get(name or "", {})
        leave = set(cfg.get("leave_alone", []))
        runs: dict[str, list[dict]] = defaultdict(list)
        for r in rows:
            runs[r.get("docket_run") or r.get("subject_ref", {}).get("run_id")].append(r)
        adm_ans = adm_act = samp_ans = samp_act = abandoned = unclosed = 0
        for run, rs in runs.items():
            asked = [r for r in rs if r.get("routed") in ("admitted", "audit-sample")]
            labels = []
            for r in asked:
                o = truth_outcome(r, decisions, outcomes)
                labels.append((r, None if o is None else o.get("label")))
            if asked and all(lbl is None for _, lbl in labels):
                unclosed += 1
                continue
            if any(lbl == "undecided" for _, lbl in labels):
                abandoned += 1
            for r, lbl in labels:
                if lbl is None or lbl == "undecided":
                    continue
                acted = lbl not in leave
                if r.get("routed") == "audit-sample":
                    samp_ans += 1
                    samp_act += int(acted)
                else:
                    adm_ans += 1
                    adm_act += int(acted)
        # Proposal agreement: the incumbent's proposed disposition vs the answer.
        disp = [d for d in decisions.values() if d.get("surface") == f"docket.disposition.{name}"
                and d.get("judge") in INCUMBENTS]
        p_ok = p_n = 0
        for d in disp:
            o = truth_outcome(d, decisions, outcomes)
            if o is None or o.get("label") in (None, "undecided"):
                continue
            p_n += 1
            p_ok += int(d.get("answer") == o.get("label"))
        failures = 0
        if ledger_dir is not None and (ledger_dir / "dockets").exists():
            for p in (ledger_dir / "dockets").glob("*.json"):
                try:
                    m = json.loads(p.read_text(encoding="utf-8"))
                except Exception:
                    continue
                if m.get("surface") == name:
                    failures += int(m.get("write_failures") or 0)
        n_runs = len([r for r in runs if r])
        out.append({
            "surface": name, "runs": n_runs, "unclosed_runs": unclosed,
            "enumerated_per_run": (len(rows) / n_runs) if n_runs else None,
            "admitted_per_run": (sum(1 for r in rows if r.get("answer") == "admitted") / n_runs) if n_runs else None,
            "action_rate": (adm_act / adm_ans) if adm_ans else None, "action_n": adm_ans,
            "abandonment_rate": (abandoned / (n_runs - unclosed)) if (n_runs - unclosed) > 0 else None,
            "miss_rate": (samp_act / samp_ans) if samp_ans else None, "miss_n": samp_ans,
            "miss_ci": C.wilson(samp_act, samp_ans), "miss_small_n": samp_ans < small,
            "proposal_agreement": (p_ok / p_n) if p_n else None, "proposal_n": p_n,
            "write_failures": failures,
        })
    return out


def retrieval_ack_metrics(decisions: dict, outcomes: dict) -> dict | None:
    """Precision of the prompt hook's injections, judged by model acks.

    A turn is one sampled prompt. Compliance = turns where Claude wrote a `mem:`
    line / turns that finished. Precision = excerpts ticked relevant / excerpts
    labeled. The used-rate by POSITION is the curve that says where a learned
    cut-off would go; by-source says which feed earns its tokens.
    """
    inj = [d for d in decisions.values() if d.get("surface") == "wiki.retrieval.injected"]
    if not inj:
        return None
    turns: dict[str, list[dict]] = defaultdict(list)
    for d in inj:
        turns[str((d.get("subject_ref") or {}).get("prompt_id"))].append(d)
    labeled: list[tuple[dict, bool, Any]] = []
    compliant = pending = undecided = explicit_none = 0
    for _turn, ds in turns.items():
        labs = []
        for d in ds:
            o = truth_outcome(d, decisions, outcomes)
            labs.append((d, None if o is None else o.get("label"), None if o is None else o.get("proxy_mentioned")))
        known = [x for x in labs if x[1] in ("used", "unused")]
        if all(lbl is None for _d, lbl, _p in labs):
            pending += 1
        elif known:
            compliant += 1
            explicit_none += int(all(lbl == "unused" for _d, lbl, _p in known))
            labeled += [(d, lbl == "used", p) for d, lbl, p in known]
        else:
            undecided += 1

    def rate(xs):
        return (sum(1 for x in xs if x[1]) / len(xs)) if xs else None

    by_pos: dict[int, list] = defaultdict(list)
    by_src: dict[str, list] = defaultdict(list)
    by_week: dict[str, list] = defaultdict(list)
    for x in labeled:
        d = x[0]
        by_pos[int(d.get("position") or 0)].append(x)
        by_src[str((d.get("subject_ref") or {}).get("source"))].append(x)
        ts = _ts(d)
        by_week[ts.strftime("%G-W%V") if ts else "?"].append(x)
    proxied = [x for x in labeled if x[2] is not None]
    used_n = sum(1 for x in labeled if x[1])
    finished = len(turns) - pending
    return {
        "turns": len(turns), "pending": pending, "finished": finished,
        "compliant": compliant, "compliance": (compliant / finished) if finished else None,
        "undecided_turns": undecided, "explicit_none_turns": explicit_none,
        "labeled": len(labeled), "used": used_n,
        "precision": rate(labeled), "precision_ci": C.wilson(used_n, len(labeled)),
        "tags_used_per_turn": (used_n / compliant) if compliant else None,
        "by_position": {p: {"n": len(v), "used_rate": rate(v)} for p, v in sorted(by_pos.items())},
        "by_source": {s: {"n": len(v), "used_rate": rate(v)} for s, v in sorted(by_src.items())},
        "by_week": {w: {"n": len(v), "used_rate": rate(v)} for w, v in sorted(by_week.items())},
        "proxy_agreement": (sum(1 for x in proxied if bool(x[2]) == x[1]) / len(proxied)) if proxied else None,
        "proxy_n": len(proxied),
    }


# ---------------------------------------------------------------------------
# Recommendations
# ---------------------------------------------------------------------------

def target_for(qtype: str, targets: dict) -> float:
    return float({"binary": targets.get("agreement_binary", 0.95),
                  "multiclass": targets.get("agreement_multiclass", 0.90),
                  "scored": targets.get("agreement_scored_within1", 0.90)}[qtype])


def recommend(g: dict, incumbent: dict | None, tables: dict) -> dict:
    t = tables.get("targets", {})
    dem = tables.get("demotion", {})
    target = target_for(g["question_type"], t)
    mode = g.get("mode") or "shadow"
    unmet: list[str] = []

    # Demotion first: it is automatic and conservative.
    demote: list[str] = []
    if g["repeat_violations"]:
        demote.append(f"{len(g['repeat_violations'])} repeatability violation(s)")
    drop = float(dem.get("agreement_drop_pts", 5)) / 100
    if g["rolling_agreement"] is not None and g["rolling_n"] >= int(dem.get("rolling_window", 50)) \
            and g["rolling_agreement"] < target - drop:
        demote.append(f"rolling-{g['rolling_n']} agreement {g['rolling_agreement']:.3f} < {target - drop:.3f}")
    if g["fail_open_rate_7d"] is not None and g["fail_open_rate_7d"] > float(dem.get("fail_open_max_7d", 0.05)):
        demote.append(f"7-day fail-open {g['fail_open_rate_7d']:.3f} > {dem.get('fail_open_max_7d', 0.05)}")
    if demote and mode in PREV_MODE:
        return {"action": "DEMOTE", "from": mode, "to": PREV_MODE[mode], "reasons": demote}

    # Promotion conditions — all required.
    if g["n_heldout"] < int(t.get("min_heldout_n", 100)):
        unmet.append(f"held-out n {g['n_heldout']} < {t.get('min_heldout_n', 100)}")
    if g["agreement_heldout"] is None or g["agreement_heldout"] < target:
        unmet.append(f"held-out agreement {_fmt(g['agreement_heldout'])} < {target:.2f}")
    if incumbent is None:
        unmet.append("no incumbent on this surface to beat")
    else:
        common = [h for h in g["heldout_hashes"] if h in incumbent["heldout_ok"]]
        if not common:
            unmet.append("no held-out items judged by both this judge and the incumbent")
        else:
            mine = sum(1 for h in common if g["heldout_ok"][h]) / len(common)
            theirs = sum(1 for h in common if incumbent["heldout_ok"][h]) / len(common)
            if mine < theirs:
                unmet.append(f"below incumbent on {len(common)} shared held-out items ({mine:.3f} < {theirs:.3f})")
    if g["ece_heldout"] is None:
        unmet.append("ECE not measured (no probabilities joined to truth)")
    elif g["ece_heldout"] > float(t.get("ece_max", 0.05)):
        unmet.append(f"ECE {g['ece_heldout']:.3f} > {t.get('ece_max', 0.05)}")
    jitter_warn = float(t.get("jitter_warn", 0.05))
    if g.get("jitter_max") is not None and g["jitter_max"] > jitter_warn:
        unmet.append(f"probability jitter up to {g['jitter_max']:.3f} > {jitter_warn} — a threshold that close to tau flips")
    if g["repeat_violations"]:
        unmet.append(f"{len(g['repeat_violations'])} repeatability violation(s)")
    elif not g["repeat_comparable"]:
        unmet.append("repeatability not comparable (sampling unknown)")
    elif g["repeat_groups"] == 0:
        unmet.append("repeatability untested (no replayed states)")
    if g["fail_open_rate"] is not None and g["fail_open_rate"] > float(t.get("fail_open_max", 0.01)):
        unmet.append(f"fail-open {g['fail_open_rate']:.3f} > {t.get('fail_open_max', 0.01)}")

    if not unmet and mode in NEXT_MODE:
        return {"action": "PROMOTE", "from": mode, "to": NEXT_MODE[mode],
                "reasons": ["all promotion conditions met — Ryan decides; this tool never applies it"]}
    return {"action": "HOLD", "from": mode, "to": mode, "reasons": unmet or ["already at gating"]}


def _fmt(x: float | None, pct: bool = False) -> str:
    if x is None:
        return "—"
    return f"{x * 100:.1f}%" if pct else f"{x:.3f}"


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def build_report(paths: list[Path] | None, retrievals: Path | None, tables: dict,
                 now: datetime | None = None) -> dict:
    now = now or datetime.now(timezone.utc)
    decisions, outcomes, stats = load(paths, retrievals)
    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for r in decisions.values():
        groups[(r["surface"], r["judge"])].append(r)
    scored = {k: score_group(v, decisions, outcomes, tables, now) for k, v in sorted(groups.items())}
    recs = []
    for (surface, judge), g in scored.items():
        if judge in INCUMBENTS or judge == "human":
            continue
        inc = scored.get((surface, "code")) or scored.get((surface, "incumbent-llm"))
        recs.append({"surface": surface, "judge": judge, **recommend(g, inc, tables)})
    ledger_dir = Path(paths[0]).parent if paths else L.ledger_path().parent
    return {
        "generated_at": now.isoformat(timespec="seconds"),
        "rows": len(decisions), "outcomes": sum(len(v) for v in outcomes.values()),
        "malformed": stats.get("malformed", 0), "retrievals": stats.get("retrievals", {}),
        "split": "held-out = last hex digit of the judged item's sha256 is odd",
        "dockets": docket_metrics(decisions, outcomes, tables, ledger_dir),
        "retrieval_acks": retrieval_ack_metrics(decisions, outcomes),
        "judges": list(scored.values()),
        "recommendations": recs,
    }


def render_markdown(rep: dict) -> str:
    out = [f"# Judgment scorecard — {rep['generated_at']}", "",
           f"{rep['rows']} decision rows · {rep['outcomes']} outcomes · {rep['malformed']} malformed line(s)",
           f"Split: {rep['split']}", ""]
    if rep["dockets"]:
        out += ["## Dockets", "",
                "| Docket | Runs | Unclosed | Enumerated/run | Admitted/run | Action rate (n) | Abandonment | Miss rate (n, 95% CI) | Proposal agreement (n) | Write failures |",
                "|---|---|---|---|---|---|---|---|---|---|"]
        for d in rep["dockets"]:
            ci = d["miss_ci"]
            miss = "—" if d["miss_rate"] is None else f"{_fmt(d['miss_rate'], True)} ({d['miss_n']}, {_fmt(ci[0], True)}–{_fmt(ci[1], True)})"
            if d["miss_small_n"] and d["miss_n"]:
                miss += " ⚠ n too small"
            out.append(f"| {d['surface']} | {d['runs']} | {d['unclosed_runs']} | {_fmt(d['enumerated_per_run'])} | "
                       f"{_fmt(d['admitted_per_run'])} | {_fmt(d['action_rate'], True)} ({d['action_n']}) | "
                       f"{_fmt(d['abandonment_rate'], True)} | {miss} | {_fmt(d['proposal_agreement'], True)} ({d['proposal_n']}) | {d['write_failures']} |")
        out.append("")
    ra = rep.get("retrieval_acks")
    if ra:
        ci = ra["precision_ci"] or (None, None)
        out += ["## Retrieval acks — what Claude used of the injected memory", "",
                f"{ra['turns']} sampled turns · {ra['compliant']} acked (compliance {_fmt(ra['compliance'], True)} of "
                f"{ra['finished']} finished) · {ra['explicit_none_turns']} said `mem: -` · {ra['undecided_turns']} undecided · "
                f"{ra['pending']} pending",
                f"**Precision: {_fmt(ra['precision'], True)}** of {ra['labeled']} labeled excerpts "
                f"(95% CI {_fmt(ci[0], True)}–{_fmt(ci[1], True)}) · {_fmt(ra['tags_used_per_turn'])} used per acked turn · "
                f"tick vs zero-token proxy agree {_fmt(ra['proxy_agreement'], True)} (n={ra['proxy_n']})", "",
                "| Position | n | Used |", "|---|---|---|"]
        out += [f"| {p} | {v['n']} | {_fmt(v['used_rate'], True)} |" for p, v in ra["by_position"].items()]
        out += ["", "| Source | n | Used |", "|---|---|---|"]
        out += [f"| {s} | {v['n']} | {_fmt(v['used_rate'], True)} |" for s, v in ra["by_source"].items()]
        out += ["", "| Week | n | Used |", "|---|---|---|"]
        out += [f"| {w} | {v['n']} | {_fmt(v['used_rate'], True)} |" for w, v in ra["by_week"].items()]
        out.append("")
    out += ["## Judges", "",
            "| Surface | Judge | Mode | Items | Joined | Held-out agreement (n) | ECE | Repeatability | Fail-open | p50/p95 ms |",
            "|---|---|---|---|---|---|---|---|---|---|"]
    for g in rep["judges"]:
        if g["repeat_violations"]:
            rep_txt = f"❌ {len(g['repeat_violations'])} violation(s)"
        elif not g["repeat_comparable"]:
            rep_txt = "not comparable (sampling unknown)"
        elif g["repeat_groups"]:
            rep_txt = f"✅ {g['repeat_groups']} group(s)"
            if g.get("jitter_max"):
                rep_txt += f", p-jitter ≤ {g['jitter_max']:.3f}"
        else:
            rep_txt = "untested"
        out.append(f"| {g['surface']} | {g['judge']} | {g['mode']} | {g['items']} | {g['joined']} | "
                   f"{_fmt(g['agreement_heldout'], True)} ({g['n_heldout']}) | {_fmt(g['ece_heldout'])} | {rep_txt} | "
                   f"{_fmt(g['fail_open_rate'], True)} | {g['latency_p50'] or '—'}/{g['latency_p95'] or '—'} |")
    out.append("")
    viol = [(g, v) for g in rep["judges"] for v in g["repeat_violations"]]
    if viol:
        out += ["## Repeatability violations", ""]
        for g, v in viol:
            out.append(f"- **{g['surface']} / {g['judge']}** `{v['state_hash']}` — {v['n']} runs, "
                       f"decisions {v['answers']} (raw {v['raw_answers']})")
        out.append("")
    cal = [g for g in rep["judges"] if g["reliability"]]
    for g in cal:
        out += [f"### Reliability — {g['surface']} / {g['judge']}", "", "| Bucket | n | Mean confidence | Observed |", "|---|---|---|---|"]
        for b in g["reliability"]:
            if b["n"]:
                out.append(f"| {b['bucket']} | {b['n']} | {_fmt(b['mean_confidence'])} | {_fmt(b['observed_accuracy'])} |")
        out.append("")
    if rep["recommendations"]:
        out += ["## Recommendations (proposals — promotion is Ryan's call)", ""]
        for r in rep["recommendations"]:
            out.append(f"- **{r['action']}** {r['surface']} / {r['judge']} ({r['from']} → {r['to']}): " + "; ".join(r["reasons"]))
        out.append("")
    rs = rep.get("retrievals") or {}
    if rs:
        out += ["## Wiki retrieval (adapter, mainbrain_fitted#313)", "",
                f"{rs.get('records', 0)} retrieval records · {rs.get('records_with_typed_hits', 0)} with typed relevance · {rs.get('typed_hits', 0)} typed hits", ""]
    return "\n".join(out)


def enforce(rep: dict, tables_path: Path) -> list[dict]:
    """Apply DEMOTE recommendations only. Never promotes, whatever the input."""
    demos = [r for r in rep["recommendations"] if r["action"] == "DEMOTE"]
    if not demos:
        return []
    tables = json.loads(tables_path.read_text(encoding="utf-8"))
    applied = []
    for r in demos:
        s = tables.setdefault("surfaces", {}).setdefault(r["surface"], {})
        if s.get("mode", "shadow") != r["from"]:
            continue
        s["mode"] = r["to"]
        entry = {"ts": L.now_iso(), "surface": r["surface"], "judge": r["judge"],
                 "from": r["from"], "to": r["to"], "reasons": r["reasons"]}
        tables.setdefault("_demotions", []).append(entry)
        applied.append(entry)
    if applied:
        tables_path.write_text(json.dumps(tables, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    return applied


def append_history(rep: dict, ledger_dir: Path) -> None:
    ledger_dir.mkdir(parents=True, exist_ok=True)
    slim = {"ts": rep["generated_at"], "rows": rep["rows"],
            "judges": [{k: g[k] for k in ("surface", "judge", "mode", "items", "n_heldout", "agreement_heldout",
                                           "ece_heldout", "fail_open_rate", "jitter_max")}
                       | {"violations": len(g["repeat_violations"])}
                       for g in rep["judges"]],
            "dockets": [{k: d[k] for k in ("surface", "runs", "action_rate", "miss_rate", "miss_n", "proposal_agreement")}
                        for d in rep["dockets"]],
            "recommendations": [{k: r[k] for k in ("surface", "judge", "action")} for r in rep["recommendations"]],
            "retrieval_acks": ({k: rep["retrieval_acks"][k] for k in ("turns", "compliance", "labeled", "precision",
                                                                       "proxy_agreement")}
                               if rep.get("retrieval_acks") else None)}
    with open(ledger_dir / "scorecard-history.jsonl", "a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(slim, ensure_ascii=False) + "\n")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Judgment scorecard: success rates, promotion/demotion proposals")
    ap.add_argument("--ledger", action="append", help="ledger file(s); repeatable. Default: every ledger*.jsonl in the default judgments dir")
    ap.add_argument("--retrievals", help="wiki retrieval ledger (default ~/.claude/hooks/logs/retrievals.jsonl)")
    ap.add_argument("--no-retrievals", action="store_true")
    ap.add_argument("--tables", help="decision-tables.json (default: beside this script)")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    ap.add_argument("--enforce", action="store_true", help="apply DEMOTIONS only; never promotes")
    ap.add_argument("--no-history", action="store_true", help="do not append to scorecard-history.jsonl")
    args = ap.parse_args(argv)

    tables_path = Path(args.tables) if args.tables else L.TABLES_PATH
    tables = L.load_tables(tables_path)
    paths = [Path(p) for p in args.ledger] if args.ledger else None
    retr = None
    if not args.no_retrievals:
        retr = Path(args.retrievals) if args.retrievals else (L._home() / ".claude" / "hooks" / "logs" / "retrievals.jsonl")
    rep = build_report(paths, retr, tables)
    if args.enforce:
        rep["demotions_applied"] = enforce(rep, tables_path)
    if not args.no_history:
        try:
            append_history(rep, Path(paths[0]).parent if paths else L.ledger_path().parent)
        except Exception as e:
            sys.stderr.write(f"[audit] history not written: {e}\n")
    if args.json:
        slim = {**rep, "judges": [{k: v for k, v in g.items() if k not in ("heldout_hashes", "heldout_ok")} for g in rep["judges"]]}
        print(json.dumps(slim, indent=2, ensure_ascii=False, default=str))
    else:
        print(render_markdown(rep))
        if args.enforce:
            applied = rep["demotions_applied"]
            print(f"\n--enforce: {len(applied)} demotion(s) applied" + "".join(f"\n  - {a['surface']} {a['from']} → {a['to']}" for a in applied))
    return 0


if __name__ == "__main__":
    sys.exit(main())
