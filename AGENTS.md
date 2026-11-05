# Agent instructions

## Draft-first CI and local gate

Open implementation PRs as drafts (`gh pr create --draft`). Commit and push the
final tree, then run `python scripts/check.py --pr NUMBER --attest`. The gate
replays the credential-free jobs listed in `scripts/local-gate.json` directly
from the committed workflow files with nektos/act and Docker; it records the
head/base SHA, command, workflow hashes and output hashes in the git directory
and posts that evidence to the PR only after success. Any head/base change
invalidates the evidence: rerun before `gh pr ready NUMBER`. Never fabricate an
attestation or mark ready after a failed/unavailable local gate.

See CONTRIBUTING-CI.md for prerequisites and hosted-only checks. Full GitHub
checks still run when ready, after pushes to ready PRs, and on the existing
integration push triggers. No required check or branch rule is removed.
