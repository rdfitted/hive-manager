"""Replay the committed credential-free GitHub gate; attest only a clean PR head."""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(args, cwd=ROOT):
    options = dict(cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                   stderr=subprocess.STDOUT, shell=False)
    if os.name == 'nt':
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0
        options.update(startupinfo=startup, creationflags=subprocess.CREATE_NO_WINDOW)
    result = subprocess.run(args, **options)
    if result.returncode:
        raise RuntimeError(f'{args[0]} failed ({result.returncode}); no attestation written.\n'
                           + result.stdout.decode('utf-8', errors='replace'))
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pr', required=True, type=int)
    parser.add_argument('--attest', action='store_true', help='Post evidence to this PR after success')
    parser.add_argument('--list', action='store_true', help='Inspect coverage without running tests')
    args = parser.parse_args()
    config = json.loads((ROOT / 'scripts/local-gate.json').read_text(encoding='utf-8'))
    if args.list:
        print(json.dumps(config, indent=2))
        return
    if not shutil.which('act'):
        raise SystemExit('Install nektos/act and start a Docker Engine first; see CONTRIBUTING-CI.md.')
    if run(['git', 'status', '--porcelain']).strip():
        raise SystemExit('Commit or remove working-tree changes before running the gate.')
    sha = run(['git', 'rev-parse', 'HEAD']).decode().strip()
    pr = json.loads(run(['gh', 'api', f'repos/{config["repository"]}/pulls/{args.pr}']))
    if pr['head']['sha'] != sha or pr['head']['repo']['full_name'] != config['repository']:
        raise SystemExit('The PR head must match the clean local HEAD in this repository.')
    if not config['jobs']:
        raise SystemExit('No credential-free local jobs configured; use the hosted gate.')
    evidence = {'sha': sha, 'base_sha': pr['base']['sha'], 'command':
                f'python scripts/check.py --pr {args.pr}', 'jobs': [], 'hosted_only': config['hosted_only']}
    gitdir = Path(run(['git', 'rev-parse', '--absolute-git-dir']).decode().strip())
    logs = gitdir / 'local-gate-logs' / sha
    logs.mkdir(parents=True, exist_ok=True)
    evidence['act_version'] = run(['act', '--version']).decode().strip()
    # Refuse hidden act configuration: it can inject mounts, credentials, or skipped jobs.
    for path in [ROOT / '.actrc', Path.home() / '.actrc',
                 Path(os.environ.get('XDG_CONFIG_HOME', str(Path.home() / '.config'))) / 'act/actrc',
                 Path.home() / 'Library/Application Support/act/actrc',
                 Path(os.environ.get('LOCALAPPDATA', str(Path.home() / 'AppData/Local'))) / 'act/actrc',
                 Path(os.environ.get('APPDATA', str(Path.home() / 'AppData/Roaming'))) / 'act/actrc']:
        if path.exists():
            raise SystemExit(f'Review/remove act configuration before replay: {path}')
    with tempfile.TemporaryDirectory(prefix='local-gate-') as temp:
        temp = Path(temp)
        event = temp / 'event.json'
        local_pr = dict(pr, draft=False)
        event.write_text(json.dumps({'action': 'ready_for_review', 'number': args.pr,
                                     'pull_request': local_pr, 'repository': pr['base']['repo']}), encoding='utf-8')
        empty = temp / 'empty'
        empty.write_text('', encoding='utf-8')
        for job in config['jobs']:
            workflow = ROOT / job['workflow']
            command = ['act', 'pull_request', '-W', str(workflow), '-j', job['job'],
                       '--eventpath', str(event), '--secret-file', str(empty),
                       '--env-file', str(empty), '--var-file', str(empty),
                       '--secret', 'GITHUB_TOKEN=', '--env', 'GITHUB_TOKEN=',
                       '--container-daemon-socket', '-',
                       '--artifact-server-path', str(temp / 'artifacts'),
                       '--container-architecture', 'linux/amd64',
                       '-P', 'ubuntu-latest=ghcr.io/catthehacker/ubuntu:act-latest',
                       '-P', 'ubuntu-24.04=ghcr.io/catthehacker/ubuntu:act-24.04']
            print(f'Replaying {job["workflow"]}: {job["job"]}', flush=True)
            output = run(command)
            # A zero exit with every job skipped is not evidence of a passing gate.
            if b'Job succeeded' not in output:
                raise RuntimeError('Replay did not report a successful job; no attestation written.')
            log = logs / (Path(job['workflow']).name + '-' + job['job'] + '.log')
            log.write_bytes(output)
            evidence['jobs'].append(dict(job, workflow_sha256=hashlib.sha256(workflow.read_bytes()).hexdigest(),
                                         output_sha256=hashlib.sha256(output).hexdigest(), log=str(log)))
        if run(['git', 'rev-parse', 'HEAD']).decode().strip() != sha or run(['git', 'status', '--porcelain']).strip():
            raise SystemExit('The checkout changed during replay; no attestation written.')
        latest = json.loads(run(['gh', 'api', f'repos/{config["repository"]}/pulls/{args.pr}']))
        if latest['head']['sha'] != sha or latest['base']['sha'] != pr['base']['sha']:
            raise SystemExit('PR head/base changed during replay; rerun the gate.')
        report = gitdir / 'local-gate-evidence.json'
        report.write_text(json.dumps(evidence, indent=2) + '\n', encoding='utf-8')
        digest = hashlib.sha256(report.read_bytes()).hexdigest()
        print(f'PASS {sha}; evidence SHA256 {digest}; {report}')
        if args.attest:
            body = temp / 'comment.md'
            body.write_text('Local gate evidence (hosted checks remain mandatory)\n\n```json\n'
                            + report.read_text(encoding='utf-8') + '\n```\nEvidence SHA256: `' + digest
                            + '`\nContainer replay verifies the workflow commands, not hosted OS/platform parity.\n', encoding='utf-8')
            run(['gh', 'pr', 'comment', str(args.pr), '--repo', config['repository'], '--body-file', str(body)])


if __name__ == '__main__':
    main()
