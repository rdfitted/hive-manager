"""slop-check: the default UI AI-ism gate.

Base pass is the Impeccable detector engine (Apache-2.0, pbakaus/impeccable),
pinned by version + SHA-256. On top of it this script applies the default layer:
false-positive filters, house bans, the gate/advisory policy, and a repo-level
ignore file whose entries must carry a reason (banned rules cannot be ignored).

Exit codes: 0 pass, 2 block (gating findings), 1 error (a target could not be scanned).
"""
from __future__ import annotations

import argparse
import fnmatch
import glob
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import urllib.request
import urllib.parse
import uuid
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

ENGINE_VERSION = "0.1.5"
# Pinned per platform. Only windows-x64 is vetted (2026-10-08: hash matches the
# GitHub release sidecar and the Authenticode signature was verified).
ENGINE_SHA256 = {
    "windows-x64": "477e544fc8880a5e82e427490cb9a5f5adab480c0e02966d33fd50772b531c71",
}
RELEASE_BASE = "https://github.com/pbakaus/impeccable/releases/download"
DEFAULT_VIEWPORTS = ["1280x800", "390x844"]
IGNORE_FILE = ".slop-check.json"

# House bans (defaults): these gate and cannot be ignored.
BANNED = [
    {"id": "ban-side-tab", "rule": "side-tab", "match": None,
     "why": "Side-tab accent stripe is banned house-wide"},
    {"id": "ban-inter", "rule": "overused-font", "match": re.compile(r"\binter\b", re.I),
     "why": "Inter is banned house-wide"},
]

SPINNER_RE = re.compile(r"animate-spin|\bspinner\b|\bloader\b|\bloading-?ring\b", re.I)
NON_UI_PATH_RE = re.compile(r"(e-?mail|mailer|newsletter|webhook)", re.I)


def platform_target() -> str:
    os_name = {"win32": "windows", "darwin": "darwin"}.get(sys.platform, "linux")
    arch = "arm64" if platform.machine().lower() in ("arm64", "aarch64") else "x64"
    return f"{os_name}-{arch}"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def ensure_engine() -> Path:
    """Return a hash-verified engine binary, downloading the pinned release if needed."""
    target = platform_target()
    expected = ENGINE_SHA256.get(target)
    if not expected:
        raise RuntimeError(f"no vetted engine pin for {target}; vet one and add it to ENGINE_SHA256")
    exe = "impeccable.exe" if target.startswith("windows") else "impeccable"
    override = os.environ.get("IMPECCABLE_BIN")
    path = Path(override) if override else Path.home() / ".impeccable" / "bin" / ENGINE_VERSION / exe
    if not path.is_file():
        if override:
            raise RuntimeError(f"IMPECCABLE_BIN points at a missing file: {path}")
        url = f"{RELEASE_BASE}/engine-v{ENGINE_VERSION}/impeccable-{target}{'.exe' if exe.endswith('.exe') else ''}"
        data = urllib.request.urlopen(url, timeout=120).read()
        if hashlib.sha256(data).hexdigest() != expected:
            raise RuntimeError(f"downloaded engine does not match the pinned hash: {url}")
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".part")
        tmp.write_bytes(data)
        tmp.replace(path)
    actual = sha256(path)
    if actual != expected:
        raise RuntimeError(f"engine hash mismatch at {path} (got {actual[:12]}, pinned {expected[:12]})")
    return path


def is_url(t: str) -> bool:
    return bool(re.match(r"^(https?|file)://", t, re.I))


def run_engine(engine: Path, target: str, viewport: str | None, allow_network: bool) -> tuple[int, list[dict], str]:
    cmd = [str(engine), "detect", target, "--json", "--no-config", "--no-inline-ignores"]
    if viewport:
        cmd += ["--viewport", viewport]
    env = dict(os.environ)
    env["IMPECCABLE_HOOK_DISABLED"] = "1"
    if not allow_network:
        # The engine's own HTTP client goes nowhere; local pages still render.
        env.update(HTTPS_PROXY="http://127.0.0.1:9", HTTP_PROXY="http://127.0.0.1:9",
                   NO_PROXY="localhost,127.0.0.1,::1")
    p = subprocess.run(cmd, capture_output=True, env=env, timeout=300)
    out = p.stdout.decode("utf-8", "replace").strip()
    try:
        findings = json.loads(out)
    except json.JSONDecodeError as exc:
        raise ValueError("engine returned invalid JSON") from exc
    if not isinstance(findings, list) or any(not isinstance(f, dict) for f in findings):
        raise ValueError("engine findings must be a list of objects")
    return p.returncode, findings, p.stderr.decode("utf-8", "replace")[-2000:]


def source_line(f: dict) -> str:
    try:
        line = int(f.get("line") or 0)
        if line <= 0 or is_url(f.get("file", "")):
            return ""
        lines = Path(f["file"]).read_text(encoding="utf-8", errors="replace").splitlines()
        return lines[line - 1] if line <= len(lines) else ""
    except OSError:
        return ""


def fp_filter(f: dict, rendered: bool) -> str | None:
    """Return a filter id when the finding is a known false-positive class."""
    rule = f.get("antipattern")
    if rule in ("border-accent-on-rounded", "side-tab") and SPINNER_RE.search(source_line(f)):
        return "F1-spinner"
    if rule == "overused-font":
        if "content_style" in source_line(f) or NON_UI_PATH_RE.search(Path(f.get("file", "")).name):
            return "F2-non-ui-font"
    if rule == "low-contrast" and not rendered:
        return "F3-static-contrast"  # static resolution can mix dark-mode tokens and light backgrounds
    return None


def banned_hit(f: dict) -> dict | None:
    for b in BANNED:
        if f.get("antipattern") == b["rule"] and (b["match"] is None or b["match"].search(f.get("snippet", ""))):
            return b
    return None


def load_ignores(root: Path | None) -> tuple[list[dict], list[str]]:
    """Read .slop-check.json; return (ignores, problems). Every ignore needs rule + reason."""
    if not root:
        return [], []
    path = root / IGNORE_FILE
    if not path.is_file():
        return [], []
    problems, ok = [], []
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        return [], [f"{IGNORE_FILE}: invalid JSON ({e})"]
    for i, ig in enumerate(data.get("ignores", [])):
        if not ig.get("rule") or not str(ig.get("reason", "")).strip():
            problems.append(f"{IGNORE_FILE} ignores[{i}]: needs both 'rule' and a non-empty 'reason'")
            continue
        ok.append(ig)
    return ok, problems


def ignore_match(f: dict, ignores: list[dict], root: Path | None) -> dict | None:
    for ig in ignores:
        if ig["rule"] != f.get("antipattern"):
            continue
        if ig.get("value") and ig["value"].lower() not in f.get("snippet", "").lower():
            continue
        if ig.get("path"):
            if not fnmatch.fnmatch(f.get("relpath", ""), ig["path"]):
                continue
        return ig
    return None


def classify(findings: list[dict], rendered: bool, viewport: str | None, ignores: list[dict], root: Path | None) -> dict:
    res = {"gating": [], "advisory": [], "suppressed": [], "ignored": []}
    for f in findings:
        f = {k: f[k] for k in ("antipattern", "name", "category", "severity", "file", "line", "snippet") if k in f}
        f.update(finding_metadata(f, rendered, root))
        if viewport:
            f["viewport"] = viewport
        ban = banned_hit(f)
        if ban:
            res["gating"].append({**f, "banned": ban["id"], "why": ban["why"]})
            continue
        fid = fp_filter(f, rendered)
        if fid:
            res["suppressed"].append({**f, "filter": fid})
            continue
        ig = ignore_match(f, ignores, root)
        if ig:
            res["ignored"].append({**f, "reason": ig["reason"]})
            continue
        gate = f.get("category") == "slop" and f.get("severity") == "warning"
        res["gating" if gate else "advisory"].append(f)
    return res


def find_root(target: str, explicit: str | None) -> Path | None:
    if explicit:
        return Path(explicit)
    if is_url(target):
        return Path.cwd()
    p = Path(target).resolve()
    for d in [p, *p.parents]:
        if (d / IGNORE_FILE).is_file() or (d / ".git").exists():
            return d
    return p if p.is_dir() else p.parent



BUCKETS = ('gating', 'advisory', 'suppressed', 'ignored', 'preexisting')


def finding_key(f: dict) -> str:
    identity = [f['antipattern'], f['relpath'], ' '.join(f.get('snippet', '').split())]
    return hashlib.sha256(json.dumps(identity, ensure_ascii=False, separators=(',', ':')).encode('utf-8')).hexdigest()


def finding_metadata(f: dict, rendered: bool, root: Path | None) -> dict:
    raw = str(f.get('file', ''))
    if not raw:
        raise ValueError('finding has no source file')
    if is_url(raw):
        parsed = urllib.parse.urlparse(raw)
        if parsed.scheme.lower() == 'file':
            raw = urllib.request.url2pathname(parsed.path)
            if parsed.netloc:
                raw = '//' + parsed.netloc + raw
        else:
            return {'relpath': raw, 'context': str(f.get('snippet', ''))[:300],
                    'finding_key': finding_key({**f, 'relpath': raw})}
    source = Path(raw).resolve()
    try:
        relpath = source.relative_to((root or Path.cwd()).resolve()).as_posix()
    except ValueError as exc:
        raise ValueError('finding source is outside --root') from exc
    context = str(f.get('snippet', ''))
    if not rendered:
        lines = source.read_text(encoding='utf-8', errors='replace').splitlines()
        line = int(f.get('line') or 0)
        if 0 < line <= len(lines):
            context = '\n'.join(lines[max(0, line - 2):line + 1])
    return {'file': str(source), 'relpath': relpath, 'context': context[:300],
            'finding_key': finding_key({**f, 'relpath': relpath})}


def apply_baseline(verdict: dict, path: str) -> None:
    base = json.loads(Path(path).read_text(encoding='utf-8'))
    if not isinstance(base, dict) or base.get('verdict_version') != 2 or base.get('verdict') not in ('pass', 'block'):
        raise ValueError('baseline must be a successful verdict v2')
    if base.get('errors') != []:
        raise ValueError('baseline must have no errors')
    keys = Counter()
    for bucket in BUCKETS:
        rows = base.get(bucket)
        if not isinstance(rows, list):
            raise ValueError('baseline is missing finding buckets')
        for f in rows:
            if (not isinstance(f, dict) or not isinstance(f.get('antipattern'), str)
                    or not f.get('antipattern') or not isinstance(f.get('relpath'), str)
                    or not f.get('relpath') or not isinstance(f.get('snippet', ''), str)):
                raise ValueError('baseline contains an invalid finding')
            rel = f['relpath']
            if not is_url(rel) and (Path(rel).is_absolute() or '..' in rel.replace('\\', '/').split('/') or '\\' in rel):
                raise ValueError('baseline finding path must be relative')
            key = finding_key(f)
            if f.get('finding_key') != key:
                raise ValueError('baseline finding key does not match its identity')
            keys[key] += 1
    for bucket in ('gating', 'advisory'):
        new = []
        for f in verdict[bucket]:
            if keys[f['finding_key']] > 0:
                keys[f['finding_key']] -= 1
                verdict['preexisting'].append(f)
            else:
                new.append(f)
        verdict[bucket] = new
    verdict['baseline'] = str(Path(path).resolve())


def timestamp() -> str:
    return datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')


def open_append(path: Path) -> int:
    if os.name != 'nt':
        return os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    # CRT O_APPEND seeks before each write on Windows and races between processes.
    # FILE_APPEND_DATA without FILE_WRITE_DATA makes the kernel append atomically.
    import ctypes
    import msvcrt
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    create = kernel.CreateFileW
    create.argtypes = (wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                       wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE)
    create.restype = wintypes.HANDLE
    handle = create(str(path), 0x0004, 0x0007, None, 4, 0x0080, None)
    if handle == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return msvcrt.open_osfhandle(handle, os.O_WRONLY | os.O_BINARY)
    except Exception:
        close = kernel.CloseHandle
        close.argtypes = (wintypes.HANDLE,)
        close(handle)
        raise


def append_row(path: str, row: dict) -> None:
    """One append-mode OS write per compact JSONL row; never retry a partial row."""
    destination = Path(path)
    if not destination.is_absolute():
        raise ValueError('--log must be an absolute path')
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = (json.dumps(row, ensure_ascii=False, separators=(',', ':')) + '\n').encode('utf-8')
    fd = open_append(destination)
    try:
        if os.write(fd, data) != len(data):
            raise OSError('short log append; row may be incomplete')
    finally:
        os.close(fd)


def log_run(a: argparse.Namespace, verdict: dict) -> dict:
    row = {'row_version': 1, 'type': 'run', 'run_id': str(uuid.uuid4()),
           'session_id': a.session, 'task_id': a.task, 'mode': a.mode, 'verdict': verdict['verdict'],
           'counts': {k: len(verdict[k]) for k in BUCKETS},
           'rules': {k: dict(Counter(f['antipattern'] for f in verdict[k])) for k in BUCKETS},
           'findings': {f['finding_key']: f['antipattern'] for k in ('gating', 'advisory') for f in verdict[k]},
           'engine_version': ENGINE_VERSION,
           'verdict_path': str(Path(a.out).resolve()) if a.out else None, 'timestamp': timestamp()}
    append_row(a.log, row)
    return row


def read_rows(pattern: str) -> list[dict]:
    matches = [pattern] if Path(pattern).is_file() else glob.glob(pattern)
    paths = sorted({str(Path(p).resolve()) for p in matches if Path(p).is_file()})
    if not paths:
        raise ValueError('--log did not match any files')
    rows = []
    for path in paths:
        for line in Path(path).read_text(encoding='utf-8').splitlines():
            row = json.loads(line)
            if not isinstance(row, dict) or row.get('row_version') != 1 or row.get('type') not in ('run', 'label'):
                raise ValueError('invalid log row')
            rows.append(row)
    return rows


def parse_time(value: str) -> datetime:
    dt = datetime.fromisoformat(value.replace('Z', '+00:00'))
    return dt.replace(tzinfo=timezone.utc) if dt.tzinfo is None else dt.astimezone(timezone.utc)


def rates(rows: list[dict], since: str | None) -> dict:
    cutoff = parse_time(since) if since else None
    runs = {r['run_id']: r for r in rows if r['type'] == 'run'
            and (cutoff is None or parse_time(r['timestamp']) >= cutoff)}
    result = {}
    for run in runs.values():
        for bucket in ('gating', 'advisory'):
            for rule, count in run['rules'][bucket].items():
                item = result.setdefault(rule, {'hits': 0, 'labeled': 0, 'fp': 0, 'tp': 0, 'fp_rate': None})
                item['hits'] += count
    labels = {}
    for row in rows:
        if row['type'] == 'label' and row['run_id'] in runs:
            key = (row['run_id'], row['finding_key'])
            if key not in labels or parse_time(row['timestamp']) >= parse_time(labels[key]['timestamp']):
                labels[key] = row
    for (run_id, key), label in labels.items():
        run = runs[run_id]
        rule = run['findings'].get(key)
        if not rule or label['label'] not in ('fp', 'tp'):
            raise ValueError('invalid label reference')
        if not sum(run['rules'][b].get(rule, 0) for b in ('gating', 'advisory')):
            continue
        item = result[rule]
        item['labeled'] += 1
        item[label['label']] += 1
    for item in result.values():
        item['fp_rate'] = item['fp'] / item['labeled'] if item['labeled'] else None
    return {'rules': dict(sorted(result.items()))}


def reporting_main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description='Local slop-check labels and rates')
    sub = ap.add_subparsers(dest='command', required=True)
    label = sub.add_parser('label')
    label.add_argument('--log', required=True)
    label.add_argument('--run', required=True)
    label.add_argument('--finding', required=True)
    label.add_argument('--label', choices=('fp', 'tp'), required=True)
    label.add_argument('--by', required=True)
    report = sub.add_parser('rates')
    report.add_argument('--log', required=True)
    report.add_argument('--since')
    a = ap.parse_args(argv)
    try:
        rows = read_rows(a.log)
        if a.command == 'rates':
            print(json.dumps(rates(rows, a.since), ensure_ascii=False))
        else:
            run = next((r for r in reversed(rows) if r['type'] == 'run' and r['run_id'] == a.run), None)
            if not run or a.finding not in run['findings'] or not a.by.strip():
                raise ValueError('label requires a known run, finding, and non-empty --by')
            row = {'row_version': 1, 'type': 'label', 'run_id': a.run, 'finding_key': a.finding,
                   'label': a.label, 'by': a.by, 'timestamp': timestamp()}
            append_row(a.log, row)
            print(json.dumps(row, ensure_ascii=False))
        return 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f'slop-check: reporting error: {exc}', file=sys.stderr)
        return 1


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in ('label', 'rates'):
        return reporting_main(argv)
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('targets', nargs='+', help='files, folders, or http(s)/file URLs')
    ap.add_argument('--viewports', default=','.join(DEFAULT_VIEWPORTS), help='URL scans only')
    ap.add_argument('--root', help='explicit relative-path and ignore root (default: nearest git root)')
    ap.add_argument('--json', '--out', dest='out', help='write the full JSON verdict here')
    ap.add_argument('--baseline', help='successful v2 verdict for the same relative tree at the base revision')
    ap.add_argument('--log')
    ap.add_argument('--session')
    ap.add_argument('--task')
    ap.add_argument('--mode', choices=('off', 'report', 'block'))
    ap.add_argument('--allow-network', action='store_true')
    ap.add_argument('--quiet', action='store_true')
    a = ap.parse_args(argv)
    if hasattr(sys.stdout, 'reconfigure'):
        sys.stdout.reconfigure(encoding='utf-8')
    verdict = {'verdict_version': 2, 'engine': {'version': ENGINE_VERSION}, 'targets': [],
               **{k: [] for k in BUCKETS}, 'errors': []}
    log_flags = (a.log, a.session, a.task, a.mode)
    if any(log_flags) and not all(log_flags):
        verdict['errors'].append('--log --session --task --mode must be supplied together')
    if a.log and not Path(a.log).is_absolute():
        verdict['errors'].append('--log must be an absolute path')
    unavailable = False
    engine = None
    if not verdict['errors']:
        try:
            engine = ensure_engine()
        except Exception as exc:
            verdict['errors'].append(f'engine unavailable: {exc}')
            unavailable = True
    if engine is not None:
        for target in a.targets:
            rendered = is_url(target)
            root = find_root(target, a.root)
            try:
                if not rendered:
                    Path(target).resolve().relative_to(root.resolve())
                ignores, problems = load_ignores(root)
                verdict['errors'].extend(problems)
                for viewport in (a.viewports.split(',') if rendered else [None]):
                    code, findings, err = run_engine(engine, target, viewport, a.allow_network)
                    verdict['targets'].append({'target': target, 'viewport': viewport,
                                               'engine_exit': code, 'raw': len(findings)})
                    if code not in (0, 2):
                        verdict['errors'].append(f'{target}: engine exit {code}: {err.strip()[-300:]}')
                        continue
                    part = classify(findings, rendered, viewport, ignores, root)
                    for bucket in part:
                        verdict[bucket].extend(part[bucket])
            except Exception as exc:
                verdict['errors'].append(f'{target}: {exc}')
        if a.baseline and not verdict['errors']:
            try:
                apply_baseline(verdict, a.baseline)
            except (OSError, ValueError, KeyError, TypeError) as exc:
                verdict['errors'].append(f'baseline: {exc}')
    verdict['verdict'] = ('unavailable' if unavailable else 'error') if verdict['errors'] else ('block' if verdict['gating'] else 'pass')
    if a.out:
        try:
            Path(a.out).write_text(json.dumps(verdict, indent=1, ensure_ascii=False), encoding='utf-8')
        except OSError as exc:
            verdict['errors'].append(f'verdict output: {exc}')
            verdict['verdict'] = 'error'
    if all(log_flags):
        try:
            log_run(a, verdict)
        except (OSError, ValueError) as exc:
            verdict['errors'].append(f'log: {exc}')
            verdict['verdict'] = 'error'
            if a.out:
                try:
                    Path(a.out).write_text(json.dumps(verdict, indent=1, ensure_ascii=False), encoding='utf-8')
                except OSError:
                    pass
    if a.quiet:
        print(f"slop-check: {verdict['verdict']}" + ''.join(f' | {k} {len(verdict[k])}' for k in BUCKETS))
    else:
        print(json.dumps(verdict, indent=1, ensure_ascii=False))
    return {'pass': 0, 'block': 2}.get(verdict['verdict'], 1)


if __name__ == '__main__':
    sys.exit(main())
