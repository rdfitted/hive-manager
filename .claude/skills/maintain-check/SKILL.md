---
name: maintain-check
description: Delta gate for code maintainability. Compares the working tree with a base revision and reports only what the change introduced - files and functions that grew past a budget, new branch-heavy functions, new lint or type suppressions, and newly duplicated blocks - then returns a pass/block verdict, a 0-100 score and exit codes for coding sessions, hive workers and CI. Use before handing off or opening a pull request for any code change, when asked to "check maintainability", "run the maintainability gate", "did this make the code worse", or when /maintain-check is invoked.
---

# /maintain-check

A deterministic gate that answers one question about a change: **did it leave the code harder to
maintain than it found it?** It measures the working tree against a base revision and gates only on
what the change introduced. Debt that was already there is listed as `preexisting` and never blocks.

Standard library only, read-only on the repository, no network.

## Run it

```text
python -I <skill>/scripts/maintain_check.py --quiet
python -I <skill>/scripts/maintain_check.py --base origin/main --json verdict.json
python -I <skill>/scripts/maintain_check.py src/a.rs src/b.ts --root <workspace> --base <sha> --json <verdict.json> --log <abs/log.jsonl> --session <id> --task <id> --mode block
```

With no file arguments it checks every file changed since the base: committed, staged, unstaged and
untracked. With no `--base` it uses the merge-base with the remote's default branch.

| Exit | Verdict | Meaning |
|---|---|---|
| 0 | `pass` | Nothing this change introduced gates. Advisories may exist; report them. |
| 2 | `block` | Gating findings. Fix them, or add a reasoned ignore, then re-run. |
| 1 | `error` / `unavailable` | The comparison could not be made (no repository, no base, bad config). Never treat as a pass. |

The verdict JSON carries `gating`, `advisory`, `ignored`, `preexisting`, `skipped`, `files`,
`base_sha`, the `budgets` in force and a `score`.

## What it checks

| Rule | Gates when | Default budget |
|---|---|---|
| `file-growth` | a file is new over the budget, crosses it, or was already over and grew past the allowance | 800 lines, allowance 20 |
| `function-length` | a function is new over the budget, crosses it, or was already over and grew past the allowance | 100 lines, allowance 5 |
| `function-branches` | the same, counting branch points (`if`, `for`, `while`, `match`, `case`, `catch`, `loop`, `&&`, `||`) | 20, allowance 2 |
| `new-suppression` | a lint or type suppression appears that the base did not have (`#[allow(...)]`, `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `# noqa`, `# type: ignore`, `# pylint: disable`) | none allowed |
| `duplicate-block` | advisory only: 8 or more significant lines added in one changed file also appear in another | 8 lines |

Growth inside the allowance is reported as `advisory`. The allowance exists because registering one
more handler in a long function should not force a refactor inside an unrelated task; anything
larger should go somewhere new.

**Score.** `score.value` is the percent of checked files with no new gating finding. Advisories
never lower it.

**Scope.** Python, Rust, TypeScript and JavaScript (including Svelte and Vue files) get every rule.
Go, Java, C#, Kotlin, Swift, Ruby and PHP get `file-growth` and `duplicate-block` only. Test files,
fixtures, vendored and generated directories are skipped, and inline Rust `#[cfg(test)]` modules are
left out of both the function table and the file's line count.

## Policy

1. **Only the delta gates.** A file nobody touched is not scanned. A long function nobody made
   longer is `preexisting`.
2. **On a block: clean up, then continue.** Move the new code to a new module or split the function,
   and re-run until the verdict is `pass`. Do not stop to ask and do not ship around the gate.
3. **Never suppress to pass.** A new suppression is itself a gating finding.
4. **`unavailable` is not `pass`.** Report it. In `block` mode, escalate.

### Budgets and reasoned ignores

A repository tunes the gate with `.maintain-check.json` at its root:

```json
{
  "budgets": {"file_lines": 1000, "function_lines": 120},
  "ignores": [
    {"rule": "file-growth", "path": "src/generated/*.ts",
     "reason": "Generated from the schema by the build; edited by the generator only"}
  ]
}
```

Budget keys: `file_lines`, `function_lines`, `function_branches`, `duplicate_lines`,
`file_growth_allowance`, `function_growth_allowance`, `branch_growth_allowance`. An ignore needs
`rule` and a non-empty `reason`; `path` (glob, repository-relative) and `symbol` (substring of the
function name) narrow it. A reason names why this code is the exception. "Intentional" alone is not
a reason. An entry without a reason is an error, not an ignore.

## Evidence, labels and rates

Passing `--log`, `--session`, `--task` and `--mode` together appends one row per run to a JSONL log.
A reviewer records a false or true positive, and the rates show which rules have earned blocking:

```text
python -I <skill>/scripts/maintain_check.py label --log <log.jsonl> --run <run_id> --finding <finding_key> --label fp --by reviewer
python -I <skill>/scripts/maintain_check.py rates --log "<glob-or-file>"
```

`rates` reports hits and the labelled false-positive rate per rule, and counts runs by verdict so a
gate that keeps returning `unavailable` is visible.

## Known limits

- Functions in Rust and TypeScript are found by matching braces after comments and strings are
  blanked out, not by a full parser. A function that is renamed and lengthened in one change reads as
  new. Regular-expression literals containing `/*` can confuse the TypeScript scan.
- Branch points are a keyword count, not a cognitive-complexity score. Ternaries are not counted.
- Duplicate detection compares changed files with each other, not with the rest of the repository.
- Import cycles and unreachable modules are out of scope; a dependency-graph tool covers those.
- Naming, cohesion and "is this in the right layer" are judgments, not measurements. They are not
  part of this verdict.

## Tests

```text
python -I -m unittest discover -s <skill>/tests
```

Add a rule or a scanner fix only with a fixture or test that reproduces the case.
