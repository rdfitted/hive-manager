# Two-sided QA checks and zero-call rescoring

Shadow only. Nothing here changes a QA verdict, routing or gating, and nothing in `src-tauri` reads these results. Promotion is the operator's call on a scorecard (#291).

The ideas come from [osolmaz/switch](https://github.com/osolmaz/switch). It isn't a dependency.

## What exists

| Tool | What it does | Model calls |
|---|---|---|
| `tools/judgment/rescore.py` | Recomputes recorded judgments under rules from `decision-tables.json` (`rescore`), or a `--rules` file | None. It imports no transport and only reads the ledger |
| `tools/judgment/shadow_qa.py` | Asks the two-sided questions over a frozen item file through `judge.py`'s gates | One request per item per pass |
| `tools/judgment/qa_two_sided_questions.json` | The `meets`, `misses` and `injection` questions | |

The rule is `P(pass) = min(P(meets), 1 - P(misses))`. All three questions go in one fan-out request, so they can't see each other's answers, and `one_sided` is the `meets` answer from the same request. That makes the comparison paired, with no jitter between the two arms.

`misses` and `injection` record on their own surfaces (`hive.qa.criterion.misses`, `.injection`). The scorecard keeps one row per item, so sharing a surface would mix them into the `meets` score.

## Results

### Synthetic bench

**These numbers don't support a conclusion.** There are 0 `hive.qa.*` rows in any ledger and 0 human labels. The bench is 80 synthetic items written by a fresh agent that wasn't told about the two-sided hypothesis. Labels are by construction, the set is not real QA work, and it's below the 200 labeled / 100 held-out targets.

Bench: `bench-v1.json` (40 pass, 40 fail), `jev-1.14.0`, 10 passes per item, first pass reported.

| Rule | n | Abstain | False pass | False fail | Accuracy | ECE |
|---|---|---|---|---|---|---|
| `one_sided` | 80 | 0 | 3 | 0 | 96.2% (77/80) | 0.049 |
| `two_sided` | 80 | 0 | 1 | 0 | 98.8% (79/80) | 0.061 |
| `one_sided_band` (0.7 / 0.3) | 80 | 2 | 1 | 0 | 98.7% (77/78) | n/a |
| `two_sided_band` (0.7 / 0.3) | 80 | 2 | 0 | 0 | 100% (78/78) | n/a |
| `one_sided`, held out | 44 | 0 | 1 | 0 | 97.7% (43/44) | 0.047 |
| `two_sided`, held out | 44 | 0 | 1 | 0 | 97.7% (43/44) | 0.080 |

- **Direction matches switch, the size can't be judged.** Two-sided fixed 2 items and broke none (sign test p = 0.50). On held-out items it changed nothing. The false pass intervals overlap: 95% Wilson 2.6-19.9% one-sided, 0.4-12.9% two-sided.
- **The false fail cost didn't show up.** Switch's false fails rose from 21 to 41. Here they stayed at 0, which more likely means the bench is too easy on that side than that the cost is absent. Real data has to answer this.
- **Calibration got slightly worse,** 0.049 to 0.061 overall, and above the 0.05 target. A `min` of two probabilities isn't a calibrated joint estimate.
- **Jitter blocks promotion for both rules.** Decisions at 0.5 never flipped in 10 passes, but the largest probability spread was 0.10 one-sided (10 of 80 items over 0.05) and 0.26 two-sided (16 of 80). The limit is 0.05. Two-sided adds jitter because either question can move the result. At the 0.7 / 0.3 band, 1 item flipped one-sided and 4 two-sided.
- **Injection:** the 2 planted items were flagged and the 2 imperative working notes weren't. The flag changes no decision.
- **Cost:** 694 input tokens per request on average, p50 94 ms, p95 133 ms. A `meets`-only request wasn't measured.

`FrozenBenchTests` in `tests/test_rescore.py` recomputes these numbers from `trace-v1.jsonl` with no key.

### Real history: two-sided didn't help

The synthetic bench's direction didn't hold up on real work. This bench is 60 items built from this repo's own git history: 58 labels are proven by history (a later fix, revert or closed issue, or behavior that survived unchanged) and 2 came from a unanimous 3-agent team. Evidence was gathered blind from the code at each commit, and none of it shares an 8-word run with the Evaluator's files. The bench is private and isn't in this repo, so `FrozenBenchTests` doesn't cover these numbers. `jev-1.14.0`, 10 passes, first pass reported.

| Rule | n | Abstain | False pass | False fail | Accuracy | ECE |
|---|---|---|---|---|---|---|
| `one_sided` | 60 | 0 | 4 | 6 | 83.3% (50/60) | 0.041 |
| `two_sided` | 60 | 0 | 4 | 8 | 80.0% (48/60) | 0.115 |
| `one_sided_band` (0.7 / 0.3) | 60 | 10 | 2 | 3 | 90.0% (45/50) | n/a |
| `two_sided_band` (0.7 / 0.3) | 60 | 10 | 1 | 4 | 90.0% (45/50) | n/a |
| `one_sided`, held out | 32 | 0 | 3 | 3 | 81.2% (26/32) | 0.071 |
| `two_sided`, held out | 32 | 0 | 3 | 4 | 78.1% (25/32) | 0.088 |

- **No false pass benefit, a false fail cost.** Two-sided broke 2 items and fixed none (sign test p = 0.50). Calibration was much worse overall.
- **Neither rule is close to promotion.** Accuracy is well under the 95% target. Decisions flipped at 0.5 on 2 items one-sided and 4 two-sided in 10 passes, and jitter reached 0.18 and 0.22.
- **The Evaluator did better where it could be compared.** On the 15 items with a matching Evaluator result, it was wrong 3 times (1 false pass, 2 false fails). One-sided was wrong 5 times (1, 4) and two-sided 6 (1, 5). That's too few to generalize.
- **The band helps more than the second question.** It's the only rule family that raised accuracy, at the price of sending 1 in 6 items to a person.

Limits: one repo, labels aren't human-confirmed, and n is below target. Most evidence is code reading, not behavior, which is harder than a QA worker's report and may explain the gap from the synthetic bench. Many requirements were reworded from fix commits.

This repo alone made two-sided look like it had no benefit. The pooled result below corrects that.

### Six repos pooled: a bias shift, at a price

The same method ran on five client repos. Their benches stay private and the results are aggregated here. The pool is 340 items and 3,400 requests, with 327 labels proven by history and 13 from a unanimous 3-agent team. Another 19 client items were blocked by redaction and never sent. `jev-1.14.0`, 10 passes, first pass reported.

| Rule | n | Abstain | False pass | False fail | Accuracy | ECE |
|---|---|---|---|---|---|---|
| `one_sided` | 340 | 0 | 11 | 18 | 91.5% (311/340) | 0.044 |
| `two_sided` | 340 | 0 | 7 | 32 | 88.5% (301/340) | 0.040 |
| `one_sided_band` (0.7 / 0.3) | 340 | 39 | 4 | 7 | 96.3% (290/301) | n/a |
| `two_sided_band` (0.7 / 0.3) | 340 | 54 | 2 | 12 | 95.1% (272/286) | n/a |
| `one_sided`, held out | 180 | 0 | 7 | 11 | 90.0% (162/180) | 0.053 |
| `two_sided`, held out | 180 | 0 | 5 | 18 | 87.2% (157/180) | 0.046 |
| `one_sided_band`, held out | 180 | 21 | 3 | 4 | 95.6% (152/159) | n/a |
| `two_sided_band`, held out | 180 | 34 | 1 | 6 | 95.2% (139/146) | n/a |

- **The shift is real in direction, and it costs more than it did in switch.** Two-sided avoided 4 false passes and added 14 false fails, about 3.5 false fails per false pass avoided. Switch's legal eval traded them 1 for 1. Two-sided was right on 4 items where one-sided was wrong, and wrong on 14 the other way (sign test p = 0.03, or 0.18 on held-out items). That p is optimistic, because items cluster by repo and many are before/after pairs of one requirement.
- **The false pass intervals overlap:** 95% Wilson 3.7-11.4% one-sided, 2.0-8.4% two-sided. False fails separate more clearly, 6.7-15.8% against 13.4-24.9%.
- **Direction varies by repo.** False passes fell in 3 of 6 repos and held in the others. False fails rose in 5 of 6 and held in 1. Accuracy was lower in 4 of 6, tied in 2.
- **Calibration is about even.** Pooled ECE is 0.044 and 0.040. The hive-manager gap didn't generalize.
- **The band beats the second question.** `one_sided_band` has fewer false passes and far fewer false fails than `two_sided`, at the cost of sending 11.5% of items to a person. `two_sided_band` has the fewest false passes, but abstains on 15.9%.
- **Nothing is ready to promote.** Plain-rule held-out accuracy is under the 95% target. Decisions flipped at 0.5 on 10 of 340 items one-sided and 14 two-sided in 10 passes, against a 100% repeatability target. Jitter reached 0.32 and 0.37, against 0.05.
- **Evaluator:** on the 19 items it covered, it was wrong 3 times, one-sided 6, two-sided 7. Too few to generalize, but Jev has not shown it beats the Evaluator.
- **Injection:** no real item was flagged. There's no labeled injection in this data, so it says nothing about recall.

Limits: the 200 labeled / 100 held-out sizes are met, but the labels aren't human-confirmed, so this can't support promotion by itself. Most evidence is code reading rather than QA-worker observations, many requirements were reworded from fix commits, and pairs reduce the real sample. These numbers come from private benches, so CI doesn't recompute them.

**Decision: don't make two-sided the default, and test `one_sided_band` next.** Two-sided pays off only if a false pass costs more than about 3.5 false fails. That may be true for a QA gate, so the weighing is the operator's. Even then, a band gets more of the benefit. All of this stays in shadow.

Replay on real recorded rows: `rescore.py --surface hive.review.finding` over the 27 recorded Jev answers recomputed every top label with 0 mismatches. 14 rows have outcomes, across two question versions, reported apart: 3/6 and 4/8 agreement. Requiring a top probability of at least 0.7 leaves 2/2 and 3/5. Those outcomes are `downstream`, not human labels, and n is too small to tune on.

## Redaction for client benches

`redaction.py` now loads known names from a local memory root through `JUDGMENT_REDACT_MEMORY_ROOT`: clients, partners, pipeline, display names, contact tables and person memories. No names are in this repo. It also blocks phone numbers and URLs on hosts that could identify someone, and a name inside an identifier like `name_orders` now matches. A hit still blocks the item; nothing is rewritten.

Limits: a name that isn't in local memory, or that's joined into camelCase, isn't caught. The check only blocks known names, so review what a new surface sends.

The operator ruled on 2026-10-30 that `hive.qa.criterion` may be sent for allowlisted client repos. That's recorded in the private root policy, not in this repo's policy, which still allows only this project. Every evidence file that was sent passes the fixed screen, including the hive-manager bench sent before the fix.

## Before real rows can be judged

`record_typed_criteria` stores the Evaluator's own `evidence` text in `observations` (`src-tauri/src/http/handlers/evaluator.rs`). Sending that shows Jev the Evaluator's reasoning, so `shadow_qa.py` takes items from a file and doesn't read incumbent rows. It also refuses text with verdict language, but that only catches explicit conclusions. Past `peer/qa-verdict.md` files have the same problem and weren't sent.

Real runs need:

1. QA-worker report excerpts in `observations` instead of the Evaluator's prose.
2. A QA-enabled hive that writes rows (#325 blocked the last one).
3. Human labels through `label.py`. Downstream outcomes follow the Evaluator's verdict, so `rescore.py` excludes them as truth for QA.

## Re-running

```bash
cd tools/judgment
python shadow_qa.py --items tests/fixtures/qa-two-sided/bench-v1.json --ledger <scratch>/ledger.jsonl --runs 10 --redact-dictionary <names>
python rescore.py --ledger <scratch>/ledger.jsonl --labels tests/fixtures/qa-two-sided/bench-v1.json
```

Each invocation sends at most 200 requests and resumes where the last stopped. The default ledger is refused. Any wording change is a new `question_version` baseline: `rescore.py` reports baselines apart and never compares them, and the frozen-trace test fails until a new trace is recorded.

## Question design (#333 rules)

These are new questions, so there's no before wording. No canonical `hive.qa.criterion` question existed in the repo. `hive.review.finding` is unchanged.

| Rule | How the questions follow it |
|---|---|
| 1. One atomic question | Three separate Nouls, combined in code. `misses` asks one thing, "unmet", with contradiction and silence as boundary cases in its criteria, instead of switch's "miss or contradict" |
| 2. Literal wording | Boundary cases are in the criteria |
| 3. Criteria extend the question | `true` is the yes case, with no double negatives |
| 4. Name state fields | `` `evidence` `` and `` `criterion.description` `` |
| 5. Minimal state | An allowlist: criterion, evidence and refs only |
| 6. Numbers stay in code | Not fully. Evidence and criteria carry raw numbers, so a budget like "p95 under 200 ms" is compared by Jev. Pre-computed `Measured` criteria are never sent |
| 7. Injection question | `hive.qa.criterion.injection`, and its criteria exclude imperative working notes |
| 8. Choice can't abstain | Not applicable, these are Nouls |
| 9. No routing on one 0.5 cut | Nothing routes. Band rules are reported beside the 0.5 rules |
| 10. Fan out | One request per pass |
| 11. Version by text | `question_version` is the question's hash, and the test pins it |

The `misses` criteria also treat words like "only" and "within" as binding. That comes from switch's qualifiers rule, which switch itself later found was likely noise. It's part of this baseline, and removing it would start a new one.

## Gather, then judge

Not built. The proposal and the measurement it needs first are on #291.
