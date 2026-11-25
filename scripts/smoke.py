"""Promotion-gate smoke for Hive Manager, a desktop app released by tag.

    python scripts/smoke.py --target staging    --base-url .
    python scripts/smoke.py --target production --base-url https://github.com/rdfitted/hive-manager

staging     builds the checkout at --base-url (a path; there is no staging deployment)
            and runs the fast headless release checks against it.
production  validates the latest published release: read-only GETs, nothing is written.
            It downloads the two updater payloads (about 36 MB), which adds one to each
            asset's public download count.

Exit 0 pass, 1 fail, 2 could not run (missing tool, env or dependency, unreachable,
out of time). The last stdout line is one JSON object; progress goes to stderr.
Contract: the maintainers' promotion-gate practice. Standard library only.
"""
import argparse
import base64
import binascii
import http.client
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = 'rdfitted/hive-manager'
RELEASES_URL = f'https://github.com/{REPOSITORY}'
PLATFORMS = ('darwin-aarch64', 'darwin-x86_64', 'windows-x86_64')
# NAMES of env vars each target needs. Keep in step with smoke.env in
# .github/promotion-gate.yml. Values are never read into output.
REQUIRED_ENV = {'staging': (), 'production': ()}
BUDGET_SECONDS = 110  # the contract is "under 2 minutes"
HTTP_TIMEOUT = 30
MAX_ASSET_BYTES = 300 * 1024 * 1024
MARKER = '<title>Hive Manager</title>'


class Failed(Exception):
    """The thing under test is wrong."""


class CouldNotRun(Exception):
    """The check could not be evaluated. Never a pass."""


def log(message):
    print(message, file=sys.stderr, flush=True)


# --- processes -------------------------------------------------------------

def _hidden():
    """Windows children get no console window and no inherited streams."""
    options: dict = dict(stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if os.name == 'nt':
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0
        options.update(startupinfo=startup, creationflags=subprocess.CREATE_NO_WINDOW)
    else:
        options.update(start_new_session=True)
    return options


def _kill_tree(process):
    if os.name == 'nt':
        subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], **_hidden())
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def run_tool(args, cwd, deadline):
    executable = shutil.which(args[0])
    if not executable:
        raise CouldNotRun(f'{args[0]} not found on PATH')
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise CouldNotRun('time budget exhausted')
    process = subprocess.Popen([executable, *args[1:]], cwd=cwd, **_hidden())
    try:
        output, _ = process.communicate(timeout=remaining)
    except subprocess.TimeoutExpired:
        _kill_tree(process)
        process.communicate()
        raise CouldNotRun(f'{args[0]} did not finish inside the time budget') from None
    return process.returncode, output.decode('utf-8', errors='replace')


def tail(text, lines=15):
    return '\n'.join(text.strip().splitlines()[-lines:])


# --- http (GET only) ---------------------------------------------------------

class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def http_get(url, deadline, follow=True, sink=None):
    """Return (status, headers, body). With sink, stream the body to that file."""
    if not url.startswith('https://'):
        raise Failed(f'refusing a non-https URL: {url}')
    opener = urllib.request.build_opener() if follow else urllib.request.build_opener(_NoRedirect)
    request = urllib.request.Request(url, headers={'User-Agent': 'promotion-gate-smoke'})
    try:
        with opener.open(request, timeout=HTTP_TIMEOUT) as response:
            if sink is None:
                return response.status, response.headers, response.read(MAX_ASSET_BYTES)
            size = 0
            with open(sink, 'wb') as handle:
                while chunk := response.read(1 << 20):
                    size += len(chunk)
                    if size > MAX_ASSET_BYTES:
                        raise Failed(f'{url} is larger than {MAX_ASSET_BYTES} bytes')
                    if time.monotonic() > deadline:
                        raise CouldNotRun('time budget exhausted during a download')
                    handle.write(chunk)
            return response.status, response.headers, size
    except urllib.error.HTTPError as error:
        if not follow and error.code in (301, 302, 303, 307, 308):
            return error.code, error.headers, b''
        if error.code == 429 or error.code >= 500:
            raise CouldNotRun(f'{url} answered {error.code}') from None
        raise Failed(f'{url} answered {error.code}') from None
    except (urllib.error.URLError, http.client.HTTPException, OSError) as error:
        raise CouldNotRun(f'{url} unreachable: {getattr(error, "reason", error)}') from None


# --- pure helpers --------------------------------------------------------------

def _b64(text, what):
    try:
        return base64.b64decode(text, validate=True)
    except (binascii.Error, ValueError):
        raise Failed(f'{what} is not base64') from None


def _key_id(raw):
    """Bytes 2-10 of a minisign key or signature, printed the way minisign prints them."""
    return raw[2:10][::-1].hex().upper()


def public_key_id(encoded):
    """Key ID of a Tauri updater pubkey (base64 of a minisign public key file)."""
    lines = _b64(encoded, 'updater pubkey').decode('utf-8', errors='replace').strip().splitlines()
    raw = _b64(lines[-1], 'updater pubkey body') if lines else b''
    if len(raw) != 42 or raw[:2] != b'Ed':
        raise Failed('updater pubkey is not a minisign Ed25519 key')
    return _key_id(raw)


def signature_key_id(encoded):
    """Key ID that made a latest.json signature (base64 of a minisign signature file)."""
    lines = _b64(encoded, 'signature').decode('utf-8', errors='replace').strip().splitlines()
    if len(lines) != 4:
        raise Failed('signature is not a four-line minisign signature')
    raw = _b64(lines[1], 'signature body')
    if len(raw) != 74 or raw[:2] not in (b'Ed', b'ED'):
        raise Failed('signature is not a minisign Ed25519 signature')
    return _key_id(raw)


def parse_ref_advertisement(data):
    """Refs from a git smart-HTTP info/refs body (pkt-lines) as {ref: sha}."""
    refs, offset = {}, 0
    while offset + 4 <= len(data):
        size = int(data[offset:offset + 4], 16)
        if size < 4:  # flush packet
            offset += 4
            continue
        line = data[offset + 4:offset + size].split(b'\0')[0].rstrip(b'\n').decode('ascii', 'replace')
        offset += size
        sha, _, ref = line.partition(' ')
        if re.fullmatch(r'[0-9a-f]{40}', sha):
            refs[ref] = sha
    return refs


def check_manifest_shape(manifest, base_url, tag):
    """Enough structure to know what to download. The full rules, signatures
    included, stay in scripts/release/validate-latest.mjs."""
    version = manifest.get('version') if isinstance(manifest, dict) else None
    if version != tag[1:]:
        raise Failed(f'latest.json version {version!r} is not the latest release {tag}')
    platforms = manifest.get('platforms')
    if not isinstance(platforms, dict) or sorted(platforms) != list(PLATFORMS):
        raise Failed('latest.json platform set is incomplete or unexpected')
    prefix = f'{base_url}/releases/download/{tag}/'
    urls = set()
    for name in PLATFORMS:
        url = platforms[name].get('url') if isinstance(platforms[name], dict) else None
        if not isinstance(url, str) or not url.startswith(prefix):
            raise Failed(f'{name} URL is not under {prefix}')
        if not re.fullmatch(r'[A-Za-z0-9._-]+', url[len(prefix):]):
            raise Failed(f'{name} URL has an unsafe asset name')
        urls.add(url)
    return sorted(urls)


def exit_code(checks):
    if any(not check['ok'] and not check.get('could_not_run') for check in checks):
        return 1
    if any(check.get('could_not_run') for check in checks) or len(checks) < 3:
        return 2
    return 0


# --- checks ----------------------------------------------------------------------

def required_env(target):
    def check():
        missing = [name for name in REQUIRED_ENV[target] if not os.environ.get(name)]
        if missing:
            raise CouldNotRun('missing env: ' + ', '.join(missing))
        return f'{len(REQUIRED_ENV[target])} required'
    return check


def read_updater_config(root):
    try:
        config = json.loads((root / 'src-tauri/tauri.conf.json').read_text(encoding='utf-8'))
    except (OSError, ValueError) as error:
        raise Failed(f'src-tauri/tauri.conf.json is unreadable: {error}') from None
    updater = config.get('plugins', {}).get('updater', {})
    return config, updater, public_key_id(updater.get('pubkey', ''))


def fetch_latest_manifest(base_url, deadline):
    _, _, body = http_get(f'{base_url}/releases/latest/download/latest.json', deadline)
    try:
        return body, json.loads(body)
    except ValueError:
        raise Failed('latest.json is not JSON') from None


def staging_checks(root, deadline, state):
    def version_sources_agree():
        code, output = run_tool(['node', 'scripts/release/check-version.mjs', '--dry-run'], root, deadline)
        if code:
            raise Failed(tail(output, 3))
        return output.strip()

    def updater_config():
        config, updater, key_id = read_updater_config(root)
        expected = [f'{RELEASES_URL}/releases/latest/download/latest.json']
        if updater.get('endpoints') != expected:
            raise Failed(f'updater endpoints are {updater.get("endpoints")}, expected {expected}')
        if config.get('bundle', {}).get('createUpdaterArtifacts') is not True:
            raise Failed('bundle.createUpdaterArtifacts must be true')
        state['key_id'] = key_id
        return f'endpoint ok, pubkey id {key_id}'

    def updater_key_continuity():
        # Installed apps only accept updates signed by the key they shipped with.
        # A pubkey that differs from the published release's signing key strands them.
        if 'key_id' not in state:
            raise CouldNotRun('needs updater-config')
        _, manifest = fetch_latest_manifest(RELEASES_URL, deadline)
        platforms = manifest.get('platforms') if isinstance(manifest, dict) else None
        if not isinstance(platforms, dict) or not platforms:
            raise Failed('published latest.json has no platforms')
        for name, entry in platforms.items():
            signed_by = signature_key_id(entry.get('signature', '') if isinstance(entry, dict) else '')
            if signed_by != state['key_id']:
                raise Failed(f'published {name} is signed by {signed_by}, this tree trusts {state["key_id"]}')
        return f'published {manifest.get("version")} is signed by this tree\'s key'

    def release_tooling_tests():
        code, output = run_tool(['node', '--test', 'scripts/release/release.test.mjs'], root, deadline)
        if code:
            raise Failed(tail(output))
        passed = re.search(r'\bpass (\d+)', output)
        return f'{passed.group(1)} passed' if passed else 'passed'

    def frontend_build():
        if not (root / 'node_modules/vite').is_dir():
            raise CouldNotRun('dependencies are not installed; run npm ci first')
        started = time.time()
        code, output = run_tool(['npm', 'run', 'build'], root, deadline)
        if code:
            raise Failed(tail(output))
        index = root / 'build/index.html'
        if not index.is_file() or index.stat().st_mtime < started - 2:
            raise Failed('the build did not write a fresh build/index.html')
        if MARKER not in index.read_text(encoding='utf-8', errors='replace'):
            raise Failed(f'build/index.html lacks {MARKER}')
        return 'build/index.html written with the app marker'

    return [('required-env', required_env('staging')),
            ('version-sources-agree', version_sources_agree),
            ('updater-config', updater_config),
            ('updater-key-continuity', updater_key_continuity),
            ('release-tooling-tests', release_tooling_tests),
            ('frontend-build', frontend_build)]


def production_checks(base_url, deadline, state, workdir):
    def need(key):
        if key not in state:
            raise CouldNotRun(f'needs an earlier check ({key})')
        return state[key]

    def latest_release_tag():
        status, headers, _ = http_get(f'{base_url}/releases/latest', deadline, follow=False)
        match = re.search(r'/releases/tag/(v\d+\.\d+\.\d+)$', headers.get('Location') or '')
        if not match:
            raise Failed(f'{base_url}/releases/latest ({status}) does not point at a vX.Y.Z release')
        state['tag'] = match.group(1)
        return state['tag']

    def latest_json():
        tag = need('tag')
        body, manifest = fetch_latest_manifest(base_url, deadline)
        state['urls'] = check_manifest_shape(manifest, base_url, tag)
        (workdir / 'latest.json').write_bytes(body)
        return f'version {manifest["version"]}, {len(PLATFORMS)} platforms'

    def release_tag_commit():
        tag = need('tag')
        _, _, body = http_get(f'{base_url}.git/info/refs?service=git-upload-pack', deadline)
        try:
            refs = parse_ref_advertisement(body)
        except ValueError:
            raise CouldNotRun('the ref advertisement could not be parsed') from None
        # An annotated tag advertises its commit as the peeled ^{} entry.
        sha = refs.get(f'refs/tags/{tag}^{{}}') or refs.get(f'refs/tags/{tag}')
        if not sha:
            raise Failed(f'tag {tag} is not on the remote')
        state['sha'] = sha
        return f'{tag} -> {sha}'

    def release_assets():
        count = 0
        for url in need('urls'):
            for asset in (url, url + '.sig'):
                name = urllib.parse.unquote(asset.rsplit('/', 1)[1])
                _, _, size = http_get(asset, deadline, sink=workdir / name)
                if not size:
                    raise Failed(f'{name} is empty')
                count += 1
        state['downloaded'] = True
        return f'{count} assets downloaded'

    def updater_signatures():
        need('downloaded')
        code, output = run_tool(
            ['node', 'scripts/release/validate-latest.mjs', '--artifacts', str(workdir),
             '--tag', state['tag'], '--repository', urllib.parse.urlparse(base_url).path.strip('/'),
             '--manifest', str(workdir / 'latest.json'), '--config', 'src-tauri/tauri.conf.json'],
            ROOT, deadline)
        if code:
            raise Failed(tail(output, 3))
        return output.strip()

    return [('required-env', required_env('production')),
            ('latest-release-tag', latest_release_tag),
            ('latest-json', latest_json),
            ('release-tag-commit', release_tag_commit),
            ('release-assets', release_assets),
            ('updater-signatures', updater_signatures)]


def git_head(root, deadline):
    try:
        code, output = run_tool(['git', 'rev-parse', 'HEAD'], root, deadline)
    except CouldNotRun:
        return None
    return output.strip() if code == 0 else None


def run_checks(checks, deadline):
    results = []
    for name, check in checks:
        started = time.monotonic()
        entry = {'name': name, 'ok': False}
        try:
            if started >= deadline:
                raise CouldNotRun('time budget exhausted')
            entry['detail'] = check()
            entry['ok'] = True
        except CouldNotRun as error:
            entry.update(could_not_run=True, detail=str(error))
        except Failed as error:
            entry['detail'] = str(error)
        except Exception as error:  # a broken smoke is a failure, never a pass
            entry['detail'] = f'{type(error).__name__}: {error}'
        entry['ms'] = round((time.monotonic() - started) * 1000)
        log(f'{"ok  " if entry["ok"] else "FAIL" if not entry.get("could_not_run") else "SKIP"} '
            f'{name} ({entry["ms"]} ms) {entry.get("detail", "")}')
        results.append(entry)
    return results


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--target', required=True, choices=['staging', 'production'])
    parser.add_argument('--base-url', required=True,
                        help='staging: path of the checkout to build; production: the repository URL')
    parser.add_argument('--expect-sha', help='also require the reported sha to equal this commit')
    args = parser.parse_args(argv)
    deadline = time.monotonic() + BUDGET_SECONDS
    state = {}
    with tempfile.TemporaryDirectory(prefix='smoke-') as temp:
        if args.target == 'production':
            checks = production_checks(args.base_url.rstrip('/'), deadline, state, Path(temp))
        else:
            location = urllib.parse.urlparse(args.base_url)
            if location.scheme in ('http', 'https'):
                root = None
            elif location.scheme == 'file':
                root = Path(urllib.request.url2pathname(location.path)).resolve()
            else:
                root = Path(args.base_url).resolve()
            if root and (root / 'src-tauri/tauri.conf.json').is_file():
                checks = staging_checks(root, deadline, state)
                state['sha'] = git_head(root, deadline)
            else:
                def no_checkout():
                    raise CouldNotRun('staging has no deployment: pass the path of a checkout as --base-url')
                checks = [('checkout', no_checkout)]
        if args.expect_sha:
            def sha_matches_expected():
                if not state.get('sha'):
                    raise CouldNotRun('the sha is unknown')
                if state['sha'] != args.expect_sha:
                    raise Failed(f'sha is {state["sha"]}, expected {args.expect_sha}')
                return state['sha']
            checks.append(('sha-matches-expected', sha_matches_expected))
        results = run_checks(checks, deadline)
    report = {'target': args.target, 'sha': state.get('sha'), 'checks': results}
    if state.get('tag'):
        report['tag'] = state['tag']
    print(json.dumps(report), flush=True)
    return exit_code(results)


if __name__ == '__main__':
    sys.exit(main())
