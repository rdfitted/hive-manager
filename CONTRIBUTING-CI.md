# CI contribution flow

Use draft PRs while iterating. Run `python scripts/check.py --pr NUMBER --list`
to inspect exact local/hosted coverage. Prerequisites: Python 3.11+, Git,
authenticated GitHub CLI, nektos/act, and a running Docker Engine with Linux
containers. Review the official act installation documentation; no tool is
automatically installed by this script. The first replay downloads Linux runner
images and the workflow's pinned tools/dependencies. Do not use production
credentials. Personal `.actrc` files are refused; automatic .env/.secrets loading
is disabled. Run from a clean, committed checkout matching the remote PR head.

Run `python scripts/check.py --pr NUMBER --attest`, then `gh pr ready NUMBER`.
The script starts every Windows child hidden with redirected streams and no
shell. Container replay uses the same workflow commands, versions, services,
and matrix definitions; it does not prove GitHub-hosted OS, permissions,
secrets or event parity. Hosted-only jobs are enumerated below and remain GH
backstops. Never describe a partial replay as a full hosted-suite pass. If the
local prerequisites or replay are unavailable, leave the PR draft and report
the blocker instead of inventing evidence. Keep signed-in machine credentials
out of runner containers.

The attestation is audit evidence, not an independent security boundary. A PR
comment cannot replace required GitHub checks. Each head/base update requires
a fresh gate. Keep GitHub checks on ready_for_review and synchronize; preserve
existing integration push checks. Do not cancel deployments or release jobs.

## Local job coverage

- .github/workflows/content-gate.yml / content-gate

## Hosted-only coverage

- .github/workflows/frontend.yml / frontend-test: credentials, hosted event policy, original conditional, or OS-specific runner
- .github/workflows/macos.yml / macos-test: credentials, hosted event policy, original conditional, or OS-specific runner
- .github/workflows/render-check.yml / all: active workflow collision; left untouched
- .github/workflows/rust-tests.yml / cargo-test: credentials, hosted event policy, original conditional, or OS-specific runner
- .github/workflows/release.yml / all: release/deploy/manual or unmodified hosted workflow

## Local replay isolation

The launcher explicitly sets an empty GITHUB_TOKEN secret/environment value to
prevent act from importing the signed-in GitHub CLI credential. Job containers
do not receive the host Docker daemon socket. macOS and Windows user act config
locations are also refused. Verify this boundary with
`python -m unittest discover -s scripts -p test_check_isolation.py`. These
controls do not turn arbitrary dependency code into a security sandbox.
