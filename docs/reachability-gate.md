# Reachability gate

Compilation and tests prove that code can run correctly; they do not prove that production code can reach it. The reachability check builds a static module graph and reports modules that are not reachable from a language entrypoint. Its CI wrapper is `tools/codegraph/ci_gate.py`, which prints every finding as `path:tier:why` and supplies the failing exit codes that the vendored analyzer does not consistently provide.

## Blocking policy

Rust reachability is a blocking check in `.github/workflows/rust-tests.yml`:

```text
python tools/codegraph/ci_gate.py --lang rs --root .
```

The reachability analysis is language-specific: the blocking invocation graphs Rust modules only. It cannot prove that Python helpers such as `verify_vendor.py` are called by a workflow. The Rust workflow therefore runs vendor verification as a separate, explicitly named blocking step before reachability. Separate steps make provenance drift and unreachable Rust code distinct CI failures.

Only a `certain` finding fails the check. `certain` means the analyzer found no importer, no entrypoint, and no repository-wide textual reference (or, for Rust, a source file is never mounted into a crate). The lower-confidence `unwired`, `likely`, and `suspect` tiers remain visible for review but do not block CI because dynamic loading, framework conventions, and incomplete static evidence can make them false positives.

An empty parse is a failure, not a clean result. A zero-module graph or a `NO <LANG> MODULES FOUND` sentinel usually means the wrong root or parser was selected. Treating that result as success would silently disable the gate.

Each analyzer subprocess has a 120-second timeout. Current graphs complete well inside that bound, while two minutes leaves substantial room for slower hosted runners and still turns a stalled scan into a named gate error. The wrapper currently runs `stats` and `dead --json` separately, rebuilding the graph twice; consolidating those upstream operations is a possible performance follow-up, not part of the repository wrapper's correctness policy.

## Worktree exclusion is correctness

The vendored analyzers skip `worktrees`, `.hive-manager`, `.claude`, and `.worktrees`, along with generated and dependency roots. Agent worktrees contain near-complete copies of the repository. If those copies enter one graph, cloned modules can appear to import each other and make genuinely unreachable production modules look reachable. Excluding them prevents false negatives; it is not merely an output-cleanliness optimization. Workflow commands must not override the analyzers' `SKIP_DIRS` sets.

## TypeScript advisory

The frontend workflow runs the same wrapper with `--lang ts --advisory`, so it reports findings without failing the job. The measured baseline is 72 modules and 15 unreached findings, all `suspect` because one unbounded dynamic import caps confidence. If that dynamic import is adjudicated, the analyzer reports three `certain` findings, and all three are false positives: two modules are imported by `.svelte` components and `src/routes/+layout.ts` is a SvelteKit convention entrypoint.

The underlying limitation is structural: the analyzer sees 69 `.ts` files but does not parse the repository's 65 `.svelte` files, leaving roughly half of the frontend import graph invisible. TypeScript reachability becomes blocking only when the analyzer has a `.svelte`-aware parser, or when the frontend import graph is otherwise made visible to it. Until that named promotion condition is met and the baseline is remeasured, `--advisory` is deliberate policy.

## Updating the analyzer

The three analyzer files are vendored from `~/.claude/tools/codegraph/`, and each provenance header records its source path, copy date, and the SHA-256 of the original upstream bytes. Manifest schema v2 records two deliberately different identities:

- `upstream_sha256` is the exact original upstream byte stream, including its original line endings. It cross-checks the provenance header.
- `normalized_sha256` is the complete vendored file after converting CRLF and lone CR line endings to LF. It detects content changes consistently even when Git materializes CRLF on Windows.

The repository authors the files with LF, but verification does not depend on checkout settings. Python line endings are not semantic, so the verifier normalizes them before hashing while retaining every other byte, including the repository provenance header.

Verify all three files and cross-check each header against the manifest with one command from the repository root:

```text
python tools/codegraph/verify_vendor.py
```

A clean run prints `OK` for all three files and `Verified 3 vendored files.` Do not patch vendored analyzer logic in this repository. Replace the copies from upstream as a unit, author them with LF line endings, update the raw-upstream and normalized-content hashes in the manifest, and keep repository-specific exit policy in `ci_gate.py`.

The verifier discovers vendored analyzers independently from their provenance headers and requires that set to match the manifest exactly. This fails closed when a vendored file is omitted from the manifest or a stale manifest entry remains. Missing or malformed manifests, unsafe paths, and invalid SHA-256 values produce concise errors rather than tracebacks.
