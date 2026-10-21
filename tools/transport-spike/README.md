# Transport comparison spike (2026-10)

Evidence only for #358. This directory is standalone: nothing is imported by the
application or its build. The decision record is `docs/transport-spike-2026-10.md`.

## B1 reproduction

Requires Python 3, Git and the installed native `codex` executable. On Windows,
pass the executable inside the npm Codex distribution, rather than `codex.ps1`.

```powershell
python tools/transport-spike/b1/probe.py --codex '<absolute native executable>' --out tools/transport-spike/b1/results.json --timeout 600
```

The probe creates a unique OS-temp directory outside this repository and runs
`git init` there. It generates version-specific schemas with
`codex app-server generate-json-schema`, then launches one worker using
`-c sqlite_home=<scratch SQLite directory> app-server --listen stdio://`.
It measures push notifications and cwd/SQLite control first, with one shell
command that prints the scratch cwd. No branch/worktree of this repository is
created. No application, package or Cargo command is required.

The existing Codex authentication/configuration is consumed by the CLI; the
probe never opens or copies the user's home configuration or transcripts.
The worker is ephemeral. The override relocates SQLite, not credentials.
The summary is an allowlist of protocol names, booleans, enum values, counts,
timings and a schema digest. Raw messages, deltas, tool output, prompts, paths,
thread/turn/account identifiers and stderr are never saved. The synthetic task
in the script is authored for this experiment, not copied from a transcript.
The scratch directory is retained locally for inspection; it is never committed.
The probe does not compare or read the live shared home, so absence of all writes
there is outside the measurement.

The first 180 s attempt timed out before initialization; the successful retry
initialized in 255 s. Keep the 600 s bound when reproducing this environment.
`--isolated-startup` initializes only with an empty scratch home, without a worker.
`--isolated-home` tests a thread using that empty home. Both diagnostic modes
remove `OPENAI_API_KEY` and `CODEX_API_KEY` from the child environment; the latter
also projects `account/read` to account-present/requires-auth booleans. No tokens
or account fields are saved. The controlled no-credential turn failed with an
authentication error. Empty homes are not the M3 isolation strategy.

## B2 gate

No B2 connection may run without a QUEEN entry in the session's `shared.md`
confirming the operator completed Op (a). At measurement time no such entry
existed. Every B2 metric is therefore `BLOCKED: operator setup`.
There is intentionally no executable T3 client: no endpoint or authorized
pairing was provided. Do not install software, create accounts, approve pairings,
open tunnels or increase access to reproduce this blocked result.

## Limits

One worker and one bounded turn are not a load test, upgrade compatibility test,
native delegation test or Claude/Cursor integration estimate. A schema capability
is explicitly distinguished from a live observation in the decision record.
