"""Standalone stdlib-only app-server measurement. Writes allowlisted summaries only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--codex', required=True, help='Native codex executable (not a shell shim)')
    ap.add_argument('--out', required=True, type=Path)
    ap.add_argument('--timeout', type=int, default=600)
    ap.add_argument('--isolated-startup', action='store_true',
                    help='Diagnostic only: empty temporary CODEX_HOME, initialize then stop; no worker/auth')
    ap.add_argument('--isolated-home', action='store_true',
                    help='Diagnostic only: empty temporary CODEX_HOME, no credential copy')
    args = ap.parse_args()
    root = Path(tempfile.mkdtemp(prefix='hive-transport-spike-')).resolve()
    project = root / 'project'
    project.mkdir()
    store = root / 'sqlite'
    store.mkdir()
    try:
        subprocess.run(['git', 'init', '--quiet', str(project)], check=True,
                       capture_output=True, timeout=60)
    except subprocess.TimeoutExpired:
        raise SystemExit('Git initialization timed out (details withheld)') from None
    # This is a throwaway git init, never a branch/worktree of the application.
    schema = root / 'schema'
    try:
        subprocess.run([args.codex, 'app-server', 'generate-json-schema', '--out', str(schema)],
                       check=True, capture_output=True, timeout=args.timeout)
    except subprocess.TimeoutExpired:
        raise SystemExit('Schema generation timed out (details withheld)') from None
    digest = hashlib.sha256()
    schema_files = sorted(schema.rglob('*.json'))
    for path in schema_files:
        digest.update(path.relative_to(schema).as_posix().encode())
        digest.update(path.read_bytes())
    try:
        version = subprocess.check_output([args.codex, '--version'], text=True, timeout=60).strip()
    except subprocess.TimeoutExpired:
        raise SystemExit('Version lookup timed out (details withheld)') from None
    child_env = os.environ.copy()
    if args.isolated_startup or args.isolated_home:
        empty_home = root / 'empty-home'
        empty_home.mkdir()
        child_env['CODEX_HOME'] = str(empty_home)
        for key in ('OPENAI_API_KEY', 'CODEX_API_KEY'):
            child_env.pop(key, None)
    proc = subprocess.Popen([args.codex, '-c', 'sqlite_home=' + json.dumps(str(store)),
                             'app-server', '--listen', 'stdio://'], cwd=project,
                            env=child_env,
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, encoding='utf-8')
    incoming = queue.Queue()
    def reader():
        try:
            for line in proc.stdout:
                try:
                    incoming.put((time.monotonic(), json.loads(line)))
                except ValueError:
                    incoming.put((time.monotonic(), {'invalid_json': True}))
        except (OSError, UnicodeError) as exc:
            incoming.put((time.monotonic(), {'reader_error': type(exc).__name__}))
        finally:
            incoming.put((time.monotonic(), {'eof': True}))
    threading.Thread(target=reader, daemon=True).start()
    stderr_counts = {key: 0 for key in ('error', 'warn', 'mcp', 'config', 'hook', 'auth')}
    def drain_stderr():
        # Diagnostic categories only: no line, path, URL, prompt or identifier escapes.
        for line in proc.stderr:
            lowered = line.lower()
            for key in stderr_counts:
                if key in lowered:
                    stderr_counts[key] += 1
    threading.Thread(target=drain_stderr, daemon=True).start()
    started = time.monotonic()
    summary = {'version': version, 'schema_files': len(schema_files),
               'schema_sha256': digest.hexdigest(), 'transport': 'stdio JSONL',
               'events': [], 'responses': [], 'cwd_matches': None,
               'command_cwd_matches': None, 'sqlite_home_contains_databases': False,
               'approval_policy_never': None, 'sandbox_danger_full_access': None}
    summary['isolated_startup_only'] = args.isolated_startup
    summary['isolated_home'] = args.isolated_startup or args.isolated_home
    if summary['isolated_home']:
        summary['api_key_environment_removed'] = True
    summary['invalid_json_lines'] = 0
    pending = {}
    thread_id = None
    turn_id = None
    turn_sent = None
    def send(method, params, request_id=None):
        msg = {'method': method, 'params': params}
        if request_id is not None:
            msg['id'] = request_id
            pending[request_id] = (method, time.monotonic())
        proc.stdin.write(json.dumps(msg) + '\n')
        proc.stdin.flush()
    def start_thread():
        send('thread/start', {'model': 'gpt-6.1-sol', 'cwd': str(project),
             'approvalPolicy': 'never', 'sandbox': 'danger-full-access',
             'baseInstructions': 'Work only in the supplied scratch directory. Do not read home directories, use network tools, spawn subagents, or modify files. No git mutations.',
             'ephemeral': True}, 2)
    try:
        send('initialize', {'clientInfo': {'name': 'hive_transport_spike', 'version': '0.1.0'}}, 1)
        deadline = started + args.timeout
        while time.monotonic() < deadline:
            try:
                at, msg = incoming.get(timeout=1)
            except queue.Empty:
                continue
            if msg.get('eof'):
                summary['stopped'] = 'server EOF'
                break
            if msg.get('invalid_json'):
                summary['invalid_json_lines'] += 1
                continue
            if msg.get('reader_error'):
                summary['reader_error_category'] = msg['reader_error']
                continue
            if 'id' in msg and 'method' not in msg:
                method, sent = pending.pop(msg['id'], ('unknown', at))
                summary['responses'].append({'method': method, 'elapsed_ms': round((at-sent)*1000, 2),
                                             'ok': 'result' in msg,
                                             'error_code': msg.get('error', {}).get('code')})
                print(json.dumps(summary['responses'][-1]), flush=True)
                if 'error' in msg:
                    summary['stopped'] = 'RPC error (details withheld)'
                    break
                result = msg.get('result', {})
                if msg['id'] == 1:
                    send('initialized', {})
                    if args.isolated_startup:
                        summary['stopped'] = 'initialize-only diagnostic complete'
                        break
                    if args.isolated_home:
                        send('account/read', {'refreshToken': False}, 4)
                    else:
                        start_thread()
                elif msg['id'] == 4:
                    summary['account_present'] = result.get('account') is not None
                    summary['requires_auth'] = result.get('requiresOpenaiAuth')
                    start_thread()
                elif msg['id'] == 2:
                    thread = result['thread']
                    thread_id = thread['id']
                    summary['cwd_matches'] = Path(thread['cwd']).resolve() == project
                    summary['approval_policy_never'] = result.get('approvalPolicy') == 'never'
                    summary['sandbox_danger_full_access'] = result.get('sandbox', {}).get('type') == 'dangerFullAccess'
                    turn_sent = time.monotonic()
                    send('turn/start', {'threadId': thread_id, 'input': [{'type': 'text',
                         'text': 'Run exactly one shell command to print the current working directory. Do not inspect files, modify anything, access network or delegate. Then reply OK.'}]}, 3)
                elif msg['id'] == 3:
                    turn_id = result['turn']['id']
                    summary['ack_has_turn_id'] = bool(turn_id)
            elif 'method' in msg:
                method = msg['method']
                params = msg.get('params', {})
                event = {'method': method, 'elapsed_ms': round((at-started)*1000, 2)}
                if method.startswith(('turn/', 'item/')):
                    event['thread_matches'] = params.get('threadId') == thread_id
                    item = params.get('item', {})
                    if item:
                        event['item_type'] = item.get('type')
                        if item.get('type') == 'commandExecution':
                            event['cwd_matches'] = Path(item.get('cwd', '')).resolve() == project
                            if method == 'item/completed':
                                summary['command_cwd_matches'] = event['cwd_matches']
                                summary['command_exit_zero'] = item.get('exitCode') == 0
                    if method == 'turn/started':
                        summary['turn_started_ms'] = round((at-turn_sent)*1000, 2)
                        summary['started_turn_id_present'] = bool(params.get('turn', {}).get('id'))
                        # The notification may precede the response; compare after the run.
                        summary['started_id_matches_ack'] = None
                        started_turn_id = params.get('turn', {}).get('id')
                    if method == 'turn/completed':
                        summary['completion_status'] = params.get('turn', {}).get('status')
                        summary['completed_turn_matches'] = params.get('turn', {}).get('id') == turn_id
                        error = params.get('turn', {}).get('error') or {}
                        # Only classify an auth failure; never save upstream error text.
                        error_text = str(error).lower()
                        summary['completion_auth_error'] = any(word in error_text for word in
                            ('unauthorized', 'authentication', 'api key', '401', 'missing bearer'))
                        summary['events'].append(event)
                        break
                # Record names and structural booleans, never deltas/text/tool output/IDs.
                summary['events'].append(event)
                if 'id' in msg:
                    summary['server_request_seen'] = True
                    proc.stdin.write(json.dumps({'id': msg['id'], 'error': {
                        'code': -32601, 'message': 'Probe does not grant requests'}}) + '\n')
                    proc.stdin.flush()
        else:
            summary['stopped'] = 'timeout'
        if 'started_turn_id' in locals():
            summary['started_id_matches_ack'] = started_turn_id == turn_id
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)
        summary['sqlite_home_contains_databases'] = bool(list(store.glob('*.sqlite')))
        summary['stderr_category_counts'] = stderr_counts
        summary['sqlite_database_count'] = len(list(store.glob('*.sqlite')))
        summary['duration_ms'] = round((time.monotonic()-started)*1000, 2)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({k: v for k, v in summary.items() if k != 'events'}, indent=2))


if __name__ == '__main__':
    main()
