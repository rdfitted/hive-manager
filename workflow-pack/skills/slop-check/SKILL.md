---
name: slop-check
description: Scan UI source or rendered pages for generated UI patterns with the pinned Impeccable detector, default bans, false-positive filters, reasoned ignores, and a diff-aware pass/block verdict. Use before handing off a UI change or when asked to check for AI-isms.
---

# slop-check

The Impeccable detector (`pbakaus/impeccable`, Apache-2.0) supplies the base scan.
This skill adds deterministic policy, fixtures, baseline comparison and local reporting.
Rule catalog: <https://impeccable.style/slop>.

## Scan

Resolve `scripts/slop_check.py` relative to this skill directory. For static scans:

```sh
python -I scripts/slop_check.py <file-or-directory> --root <repository-root> --json <verdict.json> --quiet
```

URLs render at `1280x800` and `390x844` by default; override with `--viewports`.
Use rendered scans for contrast and overflow when the app can run in a browser.
For desktop apps whose browser rendering is blank, use static scans.

| Exit | Verdict | Meaning |
|---|---|---|
| 0 | pass | No new gating findings; report advisories. |
| 2 | block | Clean up new gating findings, re-run, then continue. |
| 1 | error / unavailable | Scan or engine failed. Never a pass. |

`--json` (`--out` remains an alias) writes verdict version 2. Buckets are `gating`,
`advisory`, `suppressed`, `ignored`, and `preexisting`. Each finding has a local
absolute `file`, forward-slash `relpath`, `context` capped at 300 characters, and
`finding_key`. Keep verdicts as local evidence; do not commit them.

## Policy and ignores

Default bans are the side-tab accent stripe and Inter. Bans gate and cannot be
ignored. Slop rules at warning severity gate; other slop findings and quality
findings are advisory. Quality findings are trusted only from rendered scans.

A repository's `.slop-check.json` can narrow exceptions by rule, snippet substring
and relative path. Every ignore needs a non-empty reason that identifies the
product or reader need served; "intentional" alone is insufficient justification.
The engine's own config and inline ignores are disabled.

```json
{"ignores":[{"rule":"overused-font","value":"Plus Jakarta Sans","path":"src/**","reason":"Typeface required by the product brand guide"}]}
```

| Filter | Suppresses | Reason |
|---|---|---|
| F1-spinner | border-accent-on-rounded / side-tab on spinner, loader, animate-spin lines | Loading indicator borders are functional. |
| F2-non-ui-font | overused-font in content_style or email, mailer, newsletter, webhook files | These are not page UI. |
| F3-static-contrast | low-contrast in static scans | Static resolution can combine incompatible media conditions. |

Every filter has a reproducing fixture and test. Bans take precedence over filters.

## Baseline

Scan the same relative file tree at the base revision into a temporary directory,
using that temporary root with `--root`. Then scan the current files with the
repository root and `--baseline <base-verdict.json>`. The baseline must be a
successful v2 verdict. Findings match by rule, relative path, and whitespace
normalized snippet, ignoring line numbers. Matches move to `preexisting`, including
bans, and only remaining gating findings block. Sources outside the root fail.

## Local run log and review labels

```sh
python -I scripts/slop_check.py <paths...> --root <root> --baseline <base-verdict.json> --json <verdict.json> --log <absolute-jsonl> --session <session-id> --task <task-id> --mode report --quiet
python -I scripts/slop_check.py label --log <absolute-jsonl> --run <run-id> --finding <finding-key> --label fp --by reviewer
python -I scripts/slop_check.py rates --log <glob-or-file> --since <ISO-date-or-timestamp>
```

Supply all four scan log flags together. Modes `off`, `report`, and `block` record
workflow metadata; they preserve verdicts and exit codes. A report-mode block still
returns 2. Engine failures are logged as unavailable, with exit 1. Log failures fail
the command. Run rows carry session/task ids, UUID run ids, per-bucket counts and
per-rule counts, finding keys, engine version, evidence path and UTC timestamp.
No source paths or snippets enter the log. Each row is one append write.

Label references must identify an existing run and finding; latest label wins.
Rates join labels to selected runs and report actionable hits (gating + advisory),
labels, true positives, false positives and false-positive rate per rule. Preexisting,
suppressed and ignored findings do not count as actionable hits. Logs are local;
external judgment output never changes the verdict.

## Engine pin and tests

`ENGINE_VERSION` and `ENGINE_SHA256` pin version 0.1.5 for Windows x64.
The engine cache is `~/.impeccable/bin/<version>/`; `IMPECCABLE_BIN` overrides its
location but still requires the pinned hash. Missing engines download from the
pinned GitHub release. Other platforms are unavailable until their pins are vetted.
The engine HTTP client uses a dead proxy by default; `--allow-network` lifts this.
Local rendered scans use Edge.

To upgrade, vet the release checksum, signer and telemetry strings, update the
pin, and run the full real-engine fixture suite. Do not replace the engine with a
stub to make gate tests pass.

```sh
python -I -m unittest discover -s workflow-pack/skills/slop-check/tests
```
