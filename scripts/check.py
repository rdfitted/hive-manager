"""Replay the committed credential-free GitHub gate; attest only a clean PR head."""
import argparse
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIND_ERRORS = ('only one usage of each socket address', 'address already in use',
               'forbidden by its access permissions')


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


def free_port():
    """Ask the OS for a port unused on every interface (act binds the outbound IP): gates running side by side must not share act's 34567."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(('', 0))
        return sock.getsockname()[1]


def bind_failed(error, port):
    """True only when act itself reports it cannot bind the artifact port we chose.

    act logs the port on every normal startup ("Start server on http://addr:port"), so the port
    alone proves nothing. Its listener failure is a fatal log line:
    time="..." level=fatal msg="listen tcp addr:port: bind: Only one usage of each socket address ..."
    A fatal log makes act exit, so that line is the last thing it prints: only the final non-empty
    line is examined. Job output, including a multi-line step name that quotes the message, is
    never last.
    """
    markers = '|'.join(re.escape(marker) for marker in BIND_ERRORS)
    fatal = re.compile(rf'^time="[^"]*" level=fatal msg="listen tcp \S*:{port}: bind: [^"]*?(?:{markers})',
                       re.IGNORECASE)
    lines = [line for line in str(error).splitlines() if line.strip()]
    return bool(lines) and fatal.match(lines[-1]) is not None


def snapshot_checkout(temp, sha, base_sha, base_ref, repository):
    """Copy committed objects/files, never ignored files or source Git config."""
    snapshot = temp / 'checkout'
    run(['git', '-c', 'core.hooksPath=', '-c', 'init.templateDir=', 'clone',
         '--no-local', '--no-checkout', str(ROOT), str(snapshot)])
    prefix = ['git', '-C', str(snapshot)]
    run(prefix + ['config', 'core.hooksPath', '.git/no-hooks'])
    run(prefix + ['config', 'core.autocrlf', 'false'])
    run(prefix + ['remote', 'set-url', 'origin', f'https://github.com/{repository}.git'])
    # Base may have advanced since the operator last fetched. Fetch only into
    # the sanitized clone; no workstation auth configuration is copied to it.
    try:
        run(prefix + ['cat-file', '-e', base_sha + '^{commit}'])
    except RuntimeError:
        run(prefix + ['fetch', 'origin', base_sha])
    run(prefix + ['update-ref', 'refs/remotes/origin/' + base_ref, base_sha])
    run(prefix + ['checkout', '--detach', sha])
    if any(path.is_symlink() for path in snapshot.rglob('*')):
        raise RuntimeError('Local replay refuses symlinks that could escape its snapshot.')
    return snapshot


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
        snapshot = snapshot_checkout(temp, sha, pr['base']['sha'],
                                     pr['base']['ref'], config['repository'])
        for job in config['jobs']:
            workflow = snapshot / job['workflow']
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
            # Own artifact-server port per replay; retry once if another process wins the bind race.
            port, retry = free_port(), None
            try:
                output = run(command + ['--artifact-server-port', str(port)], cwd=snapshot)
            except RuntimeError as error:
                if not bind_failed(error, port):
                    raise
                # Keep the failed attempt's output too: evidence must show a retry happened.
                first = str(error).encode('utf-8')
                first_sha = hashlib.sha256(first).hexdigest()
                first_log = logs / (Path(job['workflow']).name + '-' + job['job'] + '.attempt1-' + first_sha[:12] + '.log')
                first_log.write_bytes(first)
                retry = {'failed_attempt_port': port, 'failed_attempt_output_sha256': first_sha,
                         'failed_attempt_log': str(first_log)}
                output = run(command + ['--artifact-server-port', str(free_port())], cwd=snapshot)
            # A zero exit with every job skipped is not evidence of a passing gate.
            if b'Job succeeded' not in output:
                raise RuntimeError('Replay did not report a successful job; no attestation written.')
            log = logs / (Path(job['workflow']).name + '-' + job['job'] + '.log')
            log.write_bytes(output)
            evidence['jobs'].append(dict(job, workflow_sha256=hashlib.sha256(workflow.read_bytes()).hexdigest(),
                                         output_sha256=hashlib.sha256(output).hexdigest(), log=str(log),
                                         **(retry or {})))
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
