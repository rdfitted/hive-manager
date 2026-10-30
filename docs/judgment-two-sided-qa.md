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

**These numbers don't support a conclusion.** Real data: 0 `hive.qa.*` rows in any ledger, and 0 human labels. The bench is 80 synthetic items written by a fresh agent that wasn't told about the two-sided hypothesis. Labels are by construction, the set is not real QA work, and it's below the 200 labeled / 100 held-out targets.

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

**Decision: keep two-sided in shadow.** Don't adopt it and don't decline it until there are human labels on real QA work.

Replay on real recorded rows: `rescore.py --surface hive.review.finding` over the 27 recorded Jev answers recomputed every top label with 0 mismatches. 14 rows have outcomes, across two question versions, reported apart: 3/6 and 4/8 agreement. Requiring a top probability of at least 0.7 leaves 2/2 and 3/5. Those outcomes are `downstream`, not human labels, and n is too small to tune on.

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
