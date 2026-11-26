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
- .github/workflows/smoke.yml / smoke: installs dependencies and builds the frontend; run scripts/smoke.py on the host for local evidence

## Promotion gate smoke

`.github/promotion-gate.yml` describes how this repo is released and rolled back.
`scripts/smoke.py` is the smoke it names. It exits 0 on pass, 1 on fail and 2 when
a check could not be evaluated, and its last stdout line is one JSON object. Exit 2
is a stop, never a pass.

- `python scripts/smoke.py --target staging --base-url .` checks a clean committed
  checkout after `npm ci`: version sources agree, the updater endpoint is unchanged,
  the committed pubkey still verifies the published release's signatures, the release
  tooling tests pass, and the frontend builds.
- `python scripts/smoke.py --target production --base-url https://github.com/rdfitted/hive-manager`
  validates the latest published release with read-only requests: it resolves the
  release tag to its commit, downloads the updater payloads, checks the release is
  published with every asset including the macOS installer, and verifies every
  signature in `latest.json` against the committed pubkey.

`.github/workflows/smoke.yml` reports `smoke/staging` on pull requests (for the head
commit) and pushes to `main`. After a release, dispatch it from the tag to get
`smoke/production` on the released commit:
`gh workflow run smoke.yml --ref vX.Y.Z -f target=production`. It asserts that this
tag and commit are the latest published release, and stops if dispatched from a branch.
The smoke does not compile the Rust app or launch it. Rust Tests, macOS Tests,
Render Check and a Release dry run cover that, and a person runs the built app
before a tag is pushed.

## Local replay isolation

The launcher explicitly sets an empty GITHUB_TOKEN secret/environment value to
prevent act from importing the signed-in GitHub CLI credential. Job containers
do not receive the host Docker daemon socket. macOS and Windows user act config
locations are also refused. Verify this boundary with
`python -m unittest discover -s scripts -p test_check_isolation.py`. These
controls do not turn arbitrary dependency code into a security sandbox.

Replay runs from a temporary committed Git clone, not the workstation working
directory. Globally ignored files and local Git auth/configuration are excluded;
required committed history and the PR base ref are preserved. Symlinks block
local replay rather than allowing reads outside the snapshot.
