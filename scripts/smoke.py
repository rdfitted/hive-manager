"""Promotion-gate smoke for Hive Manager, a desktop app released by tag.

    python scripts/smoke.py --target staging    --base-url .
    python scripts/smoke.py --target production --base-url https://github.com/rdfitted/hive-manager

staging     builds the committed checkout at --base-url (a path; there is no staging
            deployment) and runs the fast headless release checks against it.
production  validates the latest published release: GET requests only, nothing is written.

Both download the two published updater payloads (about 36 MB) to verify their
signatures, which adds one to each asset's public download count.

Exit 0 pass, 1 fail, 2 could not run (missing tool, env or dependency, unreachable,
out of time, uncommitted changes). The last stdout line is one JSON object; progress
goes to stderr. GITHUB_TOKEN is optional and only raises the GitHub API rate limit.
Contract: the maintainers' promotion-gate practice. Standard library only.
"""
import argparse
import base64
import binascii
import http.client
import io
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
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
BUDGET_SECONDS = 95  # the contract is "under 2 minutes", cleanup included
HARD_LIMIT_SECONDS = 115  # a watchdog ends the run here whatever is still blocking
HTTP_TIMEOUT = 15
MAX_BODY_BYTES = 5 * 1024 * 1024
MAX_ASSET_BYTES = 300 * 1024 * 1024
MARKER = '<title>Hive Manager</title>'
TOKEN_ENV = ('GITHUB_TOKEN', 'GH_TOKEN')  # optional, sent only to api.github.com, never to a child


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
    # No child ever sees the API token.
    options.update(env={name: value for name, value in os.environ.items() if name not in TOKEN_ENV})
    return options


def _kill_tree(process):
    try:
        if os.name == 'nt':
            subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], timeout=10, **_hidden())
        else:
            os.killpg(process.pid, signal.SIGKILL)
    except (OSError, subprocess.SubprocessError):
        pass
    try:
        process.kill()
    except OSError:
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
        try:
            process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            pass  # a stray grandchild still holds the pipe; do not wait on it
        raise CouldNotRun(f'{args[0]} did not finish inside the time budget') from None
    return process.returncode, output.decode('utf-8', errors='replace')


def tail(text, lines=15):
    return '\n'.join(text.strip().splitlines()[-lines:])


# --- http (GET only) ---------------------------------------------------------

class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class _HttpsRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.startswith('https://'):
            raise Failed(f'{req.full_url} redirects away from https')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def http_get(url, deadline, follow=True, sink=None, headers=None):
    """Return (status, headers, body). With sink, stream to that file and return its size."""
    if not url.startswith('https://'):
        raise Failed(f'refusing a non-https URL: {url}')
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise CouldNotRun('time budget exhausted')
    limit = MAX_ASSET_BYTES if sink else MAX_BODY_BYTES
    opener = urllib.request.build_opener(_HttpsRedirect if follow else _NoRedirect)
    request = urllib.request.Request(url, headers={'User-Agent': 'promotion-gate-smoke', **(headers or {})})
    try:
        with opener.open(request, timeout=min(HTTP_TIMEOUT, remaining)) as response, \
                (open(sink, 'wb') if sink else io.BytesIO()) as handle:
            size = 0
            # read1 returns after one socket read, so the deadline is checked at least
            # once per socket timeout however slowly the body arrives.
            while chunk := response.read1(1 << 16):
                size += len(chunk)
                if size > limit:
                    raise Failed(f'{url} is larger than {limit} bytes')
                if time.monotonic() > deadline:
                    raise CouldNotRun('time budget exhausted during a download')
                handle.write(chunk)
            if time.monotonic() > deadline:
                raise CouldNotRun('time budget exhausted during a download')
            return response.status, response.headers, size if sink else handle.getvalue()
    except urllib.error.HTTPError as error:
        if not follow and error.code in (301, 302, 303, 307, 308):
            return error.code, error.headers, b''
        limited = error.code in (403, 429) and error.headers.get('x-ratelimit-remaining') == '0'
        if limited or error.code == 429 or error.code >= 500:
            raise CouldNotRun(f'{url} answered {error.code}') from None
        raise Failed(f'{url} answered {error.code}') from None
    except (urllib.error.URLError, http.client.HTTPException, OSError) as error:
        raise CouldNotRun(f'{url} unreachable: {getattr(error, "reason", error)}') from None


# --- pure helpers --------------------------------------------------------------

def public_key_id(encoded):
    """Key ID of a Tauri updater pubkey, printed the way minisign prints it.

    Parsed the way the shipped updater parses it (tauri-plugin-updater, then
    minisign-verify PublicKey::decode): base64 of UTF-8 text whose first line is a
    comment and whose second line is the key. Nothing is trimmed, so a key the app
    would reject is rejected here even where the release validator is more lenient.
    Shape only; the signature checks prove the key."""
    try:
        lines = [line.removesuffix('\r') for line in
                 base64.b64decode(encoded, validate=True).decode('utf-8').split('\n')]
        raw = base64.b64decode(lines[1], validate=True)
    except (binascii.Error, ValueError, TypeError, IndexError):
        raise Failed('updater pubkey is not base64 text with the key on its second line') from None
    if len(raw) != 42 or raw[:2] != b'Ed':
        raise Failed('updater pubkey is not a minisign Ed25519 key')
    return raw[2:10][::-1].hex().upper()


def parse_ref_advertisement(data):
    """Refs from a complete git smart-HTTP info/refs body (pkt-lines) as {ref: sha}.
    Raises ValueError on a truncated or malformed body."""
    refs, offset, flushed = {}, 0, False
    while offset < len(data):
        if offset + 4 > len(data):
            raise ValueError('truncated packet header')
        size = int(data[offset:offset + 4].decode('ascii'), 16)
        if size == 0:  # flush packet
            offset, flushed = offset + 4, True
            continue
        if size < 4 or offset + size > len(data):
            raise ValueError('truncated packet')
        line = data[offset + 4:offset + size].split(b'\0')[0].rstrip(b'\n').decode('ascii', 'replace')
        offset, flushed = offset + size, False
        sha, _, ref = line.partition(' ')
        if re.fullmatch(r'[0-9a-f]{40}', sha):
            refs[ref] = sha
    if not flushed:
        raise ValueError('advertisement does not end with a flush packet')
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
        asset = url[len(prefix):]
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*', asset) or asset == 'latest.json':
            raise Failed(f'{name} URL has an unsafe asset name')
        urls.add(url)
    return sorted(urls)


def check_inventory(release, tag, sizes):
    """The published release holds every file users and the updater fetch.
    sizes maps each downloaded asset name to the byte count received."""
    if not isinstance(release, dict) or release.get('tag_name') != tag:
        raise Failed(f'the release API did not return {tag}')
    if release.get('draft') or release.get('prerelease'):
        raise Failed(f'{tag} is a draft or a prerelease')
    assets = {a['name']: a for a in release.get('assets') or [] if isinstance(a, dict) and isinstance(a.get('name'), str)}

    def uploaded(name):
        asset = assets.get(name)
        if not asset or asset.get('state') != 'uploaded' or not asset.get('size'):
            raise Failed(f'{name} is missing or empty in release {tag}')
        return asset

    for name, size in sizes.items():
        if uploaded(name)['size'] != size:
            raise Failed(f'{name} downloaded {size} bytes, the release lists {assets[name]["size"]}')
    # The macOS installer is published beside the updater payloads, not in latest.json.
    installers = [name for name in assets if name.endswith('.dmg')]
    if len(installers) != 1:
        raise Failed(f'expected one macOS .dmg installer in {tag}, found {len(installers)}')
    uploaded(installers[0])
    return f'{len(assets)} assets, published, installer {installers[0]}'


def exit_code(checks):
    if any(not check['ok'] and not check.get('could_not_run') for check in checks):
        return 1
    if any(check.get('could_not_run') for check in checks) or len(checks) < 3:
        return 2
    return 0


# --- published release (shared by both targets) ---------------------------------

def resolve_latest_tag(base_url, deadline):
    status, headers, _ = http_get(f'{base_url}/releases/latest', deadline, follow=False)
    match = re.fullmatch(re.escape(base_url) + r'/releases/tag/(v\d+\.\d+\.\d+)', headers.get('Location') or '')
    if not match:
        raise Failed(f'{base_url}/releases/latest ({status}) does not point at a vX.Y.Z release of this repository')
    return match.group(1)


def download_manifest(base_url, tag, deadline, workdir):
    """Save latest.json into workdir; return the payload URLs and {name: size}."""
    _, _, body = http_get(f'{base_url}/releases/latest/download/latest.json', deadline)
    try:
        manifest = json.loads(body)
    except ValueError:
        raise Failed('latest.json is not JSON') from None
    urls = check_manifest_shape(manifest, base_url, tag)
    (workdir / 'latest.json').write_bytes(body)
    return urls, {'latest.json': len(body)}


def download_assets(urls, deadline, workdir):
    sizes = {}
    for url in urls:
        for asset in (url, url + '.sig'):
            name = asset.rsplit('/', 1)[1]
            _, _, sizes[name] = http_get(asset, deadline, sink=workdir / name)
            if not sizes[name]:
                raise Failed(f'{name} is empty')
    return sizes


def verify_signatures(base_url, tag, workdir, tree, deadline):
    """Run tree's release validator with tree's pubkey over the downloaded release."""
    code, output = run_tool(
        ['node', 'scripts/release/validate-latest.mjs', '--artifacts', str(workdir), '--tag', tag,
         '--repository', urllib.parse.urlparse(base_url).path.strip('/'),
         '--manifest', str(workdir / 'latest.json'), '--config', 'src-tauri/tauri.conf.json'],
        tree, deadline)
    if code:
        raise Failed(tail(output, 3))
    return output.strip()


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


def staging_checks(root, deadline, state, workdir):
    def checkout_commit():
        code, output = run_tool(['git', 'rev-parse', 'HEAD'], root, deadline)
        if code or not re.fullmatch(r'[0-9a-f]{40}', output.strip()):
            raise CouldNotRun('the checkout has no readable git HEAD')
        code, changes = run_tool(['git', 'status', '--porcelain'], root, deadline)
        if code or changes.strip():
            raise CouldNotRun('uncommitted changes: the sha would not describe what was built')
        state['sha'] = output.strip()
        return state['sha']

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
        return f'endpoint ok, pubkey id {key_id}'

    def updater_key_continuity():
        # Installed apps only accept updates signed by the key they shipped with. Prove
        # this tree's pubkey still verifies the published payloads, with real signatures.
        tag = resolve_latest_tag(RELEASES_URL, deadline)
        urls, _ = download_manifest(RELEASES_URL, tag, deadline, workdir)
        download_assets(urls, deadline, workdir)
        verify_signatures(RELEASES_URL, tag, workdir, root, deadline)
        return f'published {tag} verifies with this tree\'s pubkey and validator'

    def release_tooling_tests():
        code, output = run_tool(['node', '--test', 'scripts/release/release.test.mjs'], root, deadline)
        if code:
            raise Failed(tail(output))
        passed = re.search(r'\bpass (\d+)', output)
        return f'{passed.group(1)} passed' if passed else 'passed'

    def frontend_build():
        if not (root / 'node_modules/vite').is_dir():
            raise CouldNotRun('dependencies are not installed; run npm ci first')
        index = root / 'build/index.html'
        index.unlink(missing_ok=True)  # ignored build output; a stale copy must not count
        code, output = run_tool(['npm', 'run', 'build'], root, deadline)
        if code:
            raise Failed(tail(output))
        if not index.is_file():
            raise Failed('the build did not write build/index.html')
        if MARKER not in index.read_text(encoding='utf-8', errors='replace'):
            raise Failed(f'build/index.html lacks {MARKER}')
        return 'build/index.html written with the app marker'

    return [('required-env', required_env('staging')),
            ('checkout-commit', checkout_commit),
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
        state['tag'] = resolve_latest_tag(base_url, deadline)
        return state['tag']

    def latest_json():
        state['urls'], state['sizes'] = download_manifest(base_url, need('tag'), deadline, workdir)
        return f'version {state["tag"][1:]}, {len(PLATFORMS)} platforms'

    def release_tag_commit():
        tag = need('tag')
        _, _, body = http_get(f'{base_url}.git/info/refs?service=git-upload-pack', deadline)
        try:
            refs = parse_ref_advertisement(body)
        except ValueError as error:
            raise CouldNotRun(f'the ref advertisement could not be parsed: {error}') from None
        # An annotated tag advertises the object it peels to as the ^{} entry.
        sha = refs.get(f'refs/tags/{tag}^{{}}') or refs.get(f'refs/tags/{tag}')
        if not sha:
            raise Failed(f'tag {tag} is not on the remote')
        state['sha'] = sha
        return f'{tag} -> {sha}'

    def release_assets():
        sizes = download_assets(need('urls'), deadline, workdir)
        state['sizes'].update(sizes)
        state['downloaded'] = True
        return f'{len(sizes)} assets downloaded'

    def release_inventory():
        need('downloaded')
        location = urllib.parse.urlparse(base_url)
        if location.netloc != 'github.com':
            raise CouldNotRun('the release inventory needs a github.com repository URL')
        token = next((os.environ[name] for name in TOKEN_ENV if os.environ.get(name)), None)
        headers = {'Accept': 'application/vnd.github+json'}
        if token:
            headers['Authorization'] = f'Bearer {token}'
        # No redirects: the token must not follow one to another host.
        url = f'https://api.github.com/repos/{location.path.strip("/")}/releases/tags/{state["tag"]}'
        status, _, body = http_get(url, deadline, follow=False, headers=headers)
        if status != 200:
            raise Failed(f'the release API answered {status}')
        try:
            release = json.loads(body)
        except ValueError:
            raise Failed('the release API did not return JSON') from None
        return check_inventory(release, state['tag'], state['sizes'])

    def updater_signatures():
        need('downloaded')
        read_updater_config(ROOT)  # the committed pubkey must parse the way the app parses it
        return verify_signatures(base_url, state['tag'], workdir, ROOT, deadline)

    return [('required-env', required_env('production')),
            ('latest-release-tag', latest_release_tag),
            ('latest-json', latest_json),
            ('release-tag-commit', release_tag_commit),
            ('release-assets', release_assets),
            ('release-inventory', release_inventory),
            ('updater-signatures', updater_signatures)]


def expectation(state, key, expected):
    def check():
        if not state.get(key):
            raise CouldNotRun(f'the {key} is unknown')
        if state[key] != expected:
            raise Failed(f'{key} is {state[key]}, expected {expected}')
        return state[key]
    return check


def place_after(checks, anchors, check):
    index = next((i for i, (name, _) in enumerate(checks) if name in anchors), len(checks) - 1)
    checks.insert(index + 1, check)


def run_checks(checks, deadline, results=None):
    results = [] if results is None else results
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
    parser.add_argument('--expect-tag', help='production: also require the latest release to be this tag')
    args = parser.parse_args(argv)
    started = time.monotonic()
    deadline = started + BUDGET_SECONDS
    state, results = {}, []

    def report():
        document = {'target': args.target, 'sha': state.get('sha'), 'checks': results}
        if state.get('tag'):
            document['tag'] = state['tag']
        print(json.dumps(document), flush=True)

    def hard_stop():
        # Something is blocking past every per-check limit. Report it and leave.
        results.append({'name': 'time-limit', 'ok': False, 'could_not_run': True,
                        'detail': f'still running after {HARD_LIMIT_SECONDS} s; stopped',
                        'ms': round((time.monotonic() - started) * 1000)})
        report()
        os._exit(exit_code(results))  # a failure already recorded still outranks could-not-run

    watchdog = threading.Timer(HARD_LIMIT_SECONDS, hard_stop)
    watchdog.daemon = True
    watchdog.start()
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
                checks = staging_checks(root, deadline, state, Path(temp))
            else:
                def no_checkout():
                    raise CouldNotRun('staging has no deployment: pass the path of a checkout as --base-url')
                checks = [('checkout', no_checkout)]
        # Right after the check that learns the value, so a slow later check cannot starve them.
        if args.expect_sha:
            place_after(checks, ('checkout-commit', 'release-tag-commit'),
                        ('sha-matches-expected', expectation(state, 'sha', args.expect_sha)))
        if args.expect_tag:
            place_after(checks, ('latest-release-tag',),
                        ('tag-matches-expected', expectation(state, 'tag', args.expect_tag)))
        run_checks(checks, deadline, results)
    watchdog.cancel()
    report()
    return exit_code(results)


if __name__ == '__main__':
    sys.exit(main())
