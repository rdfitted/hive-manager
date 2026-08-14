#!/usr/bin/env python3
# Vendored from C:/Users/RDuff/.claude/tools/codegraph/codegraph.py on 2026-08-14; source SHA-256 8B5AE7CD70BD4AC2A5AF11D2A05AEE9CD608AF8E00A545C9B65D52EF53E77A0C; upstream commit unavailable (local source is not a Git worktree).
"""codegraph — deterministic module-dependency graph for a repo. READ-ONLY.

Phase 1: Python only. stdlib `ast`, zero dependencies.

CONTRACT (from the spec):
  * Deterministic. No LLM extraction anywhere. Parsers only.
  * Read-only on the target repo. This tool never writes into it.
  * Never silently drop an edge -- unresolved imports are RECORDED as
    unresolved, because an incomplete graph that looks complete is the
    failure that makes people stop trusting it.
  * Report, never delete. No write authority over source, ever.

The dominant risk is false-positive "dead" findings, so `dead` reports in
confidence tiers and only `certain` is ever suggested for deletion. The
acceptance bar is that `certain` is 100% true positives on human review.

Usage:
    python codegraph.py build   --root <repo> [--out graph.json]
    python codegraph.py dead    --root <repo> [--tier certain|likely|suspect]
    python codegraph.py touch   --root <repo> <path>
    python codegraph.py stats   --root <repo>
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import re
import sys
from collections import defaultdict, deque
from pathlib import Path

# Directories never walked. Keep this list explicit and small.
#
# `worktrees` / `.claude` / `.hive-manager` are not hygiene -- they are
# correctness. A repo with agent worktrees carries near-complete COPIES of
# itself; on one real repo 1228 of 1287 modules were worktree duplicates. Those
# copies import each other, so a module that is genuinely dead in the real tree
# looks alive because its own clone imports it. Walking them does not just
# inflate counts, it silently suppresses true findings.
SKIP_DIRS = {
    ".git", "__pycache__", "node_modules", ".venv", "venv", "env",
    ".mypy_cache", ".pytest_cache", ".ruff_cache", "dist", "build",
    ".next", ".turbo", "site-packages", ".tox", "htmlcov", "uploads",
    "logs", ".idea", ".vscode",
    "worktrees", ".claude", ".hive-manager", ".worktrees",
    # Agent session artifacts: transcripts, plans and task files that name
    # modules in PROSE. They are not code and not source of truth, but their
    # mentions are enough to demote a true finding from `certain` to `likely`.
    ".hive", ".swarm", ".agent-sessions",
}

# Machinery that can reach a module with NO static import edge.
#
# Deliberately NARROW. v1 also listed getattr/pkgutil/entry_points, which
# downgraded every unreached module to `suspect` and left `certain` empty --
# i.e. the tool said nothing. Inspecting the actual call sites showed every
# getattr hit was ordinary attribute access (`getattr(obj, key, None)`), not
# module loading. A false signal that silences the whole report is worse than
# no signal, so only true module-loading calls count here.
#
# And a dynamic import with a LITERAL argument is statically resolvable -- it
# becomes a real edge in parse_imports(), not an unknown. Only NON-literal
# targets (f-strings, variables) actually defeat static analysis.
# Sentinel for `f"{__name__}..."` -- build() swaps in the importing module's
# own dotted name, which is the only place that is known.
SELF_PREFIX = "<self> "

DYNAMIC_PATTERNS = [
    (re.compile(r"\bimportlib\.import_module\s*\("), "importlib.import_module"),
    (re.compile(r"(?<!\.)\bimport_module\s*\("), "import_module"),
    (re.compile(r"\b__import__\s*\("), "__import__"),
    (re.compile(r"\bpkgutil\.(walk_packages|iter_modules)\s*\("), "pkgutil-scan"),
]


# --------------------------------------------------------------------------
# discovery
# --------------------------------------------------------------------------
def walk_py(root: Path):
    """Yield every .py file under root, skipping vendored/build dirs."""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if fn.endswith(".py"):
                yield Path(dirpath) / fn


def module_name_for(path: Path, root: Path) -> str:
    """Dotted module name, package-aware.

    A directory counts as a package only if it has __init__.py; otherwise the
    directory is treated as a source root (common for `app/` layouts without a
    top-level package). Getting this wrong is the #1 cause of unresolvable
    imports, which in turn manufactures fake orphans.
    """
    rel = path.relative_to(root)
    parts = list(rel.parts)
    if parts[-1] == "__init__.py":
        parts = parts[:-1]
    else:
        parts[-1] = parts[-1][:-3]
    return ".".join(parts)


# --------------------------------------------------------------------------
# parsing
# --------------------------------------------------------------------------
def parse_imports(path: Path):
    """Return (imports, syntax_error).

    imports: list of (module_str, level) where level>0 means relative.
    Never raises -- a file we cannot parse is recorded, not skipped silently.
    """
    try:
        src = path.read_bytes().decode("utf-8", errors="replace")
        tree = ast.parse(src, filename=str(path))
    except SyntaxError as exc:
        return [], f"SyntaxError: {exc}", []
    except Exception as exc:  # noqa: BLE001 - report, never crash the run
        return [], f"{type(exc).__name__}: {exc}", []

    out = []
    dynamic_unresolvable = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                out.append((alias.name, 0))
        elif isinstance(node, ast.ImportFrom):
            # `from . import x` has module=None
            base = node.module or ""
            lvl = node.level or 0
            out.append((base, lvl))
            # `from app.routes import auth, users` imports SUBMODULES, not
            # symbols. Recording only the package `app.routes` was manufacturing
            # a fake orphan for every route and crud module in the repo -- the
            # single largest false-positive source found in phase 1. Emit each
            # alias as its own candidate; resolve() falls back to the package
            # when the alias is really just a symbol, so this is safe either way.
            for alias in node.names:
                if alias.name == "*":
                    continue
                out.append((f"{base}.{alias.name}" if base else alias.name, lvl))
        elif isinstance(node, ast.Call):
            # importlib.import_module("x.y") / import_module("x.y") / __import__("x")
            fn = node.func
            name = (fn.attr if isinstance(fn, ast.Attribute)
                    else fn.id if isinstance(fn, ast.Name) else None)
            if name in {"import_module", "__import__"} and node.args:
                arg = node.args[0]
                if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                    # LITERAL target -> a real, resolvable edge
                    out.append((arg.value, 0))
                else:
                    # non-literal (f-string, variable) -> genuinely opaque.
                    # Recover the constant lead of an f-string when there is
                    # one: import_module(f"plugins.{x}") can only ever reach
                    # `plugins.*`, so it must not cast doubt on the whole repo.
                    prefix = ""
                    if isinstance(arg, ast.JoinedStr):
                        lead = arg.values[0] if arg.values else None
                        if isinstance(lead, ast.Constant) and isinstance(lead.value, str):
                            prefix = lead.value
                        elif (isinstance(lead, ast.FormattedValue)
                              and isinstance(lead.value, ast.Name)
                              and lead.value.id == "__name__"):
                            # import_module(f"{__name__}.{name}") -- the plugin
                            # loader idiom. `__name__` is the importing package
                            # itself, so the reach IS bounded; build() swaps in
                            # the real dotted name. Left unresolved, this one
                            # call capped an entire repo at `suspect`.
                            prefix = SELF_PREFIX
                    dynamic_unresolvable.append((prefix, node.lineno))
            elif name in {"iter_modules", "walk_packages"} and node.args:
                # pkgutil.iter_modules(pkg.__path__) imports everything under
                # `pkg`. The argument names the package outright, so this reach
                # is bounded and recoverable -- treat it like an f-string lead.
                arg = node.args[0]
                prefix = ""
                if isinstance(arg, ast.Name) and arg.id == "__path__":
                    # bare `__path__` inside a package __init__ -- the package
                    # scanning ITSELF, the commonest plugin-registry shape.
                    # Only the attribute form `pkg.__path__` was handled, so
                    # this one call left a whole repo capped at `suspect`.
                    prefix = SELF_PREFIX
                elif isinstance(arg, ast.Attribute) and arg.attr == "__path__":
                    parts, cur = [], arg.value
                    while isinstance(cur, ast.Attribute):
                        parts.append(cur.attr)
                        cur = cur.value
                    if isinstance(cur, ast.Name):
                        parts.append(cur.id)
                        prefix = ".".join(reversed(parts)) + "."
                dynamic_unresolvable.append((prefix, node.lineno))
    return out, None, dynamic_unresolvable


def resolve(mod: str, level: int, importer_module: str, index: dict,
            importer_is_pkg: bool = False) -> str | None:
    """Resolve an import to an internal module name, or None if external.

    index maps dotted-module -> relpath. We try the full dotted name first,
    then progressively shorter prefixes, because `from app.models.user import
    User` may name a symbol inside app/models/user.py OR a submodule.
    """
    if level:
        # Relative import. `.` means the containing package -- which for a
        # module a/b/c.py is `a.b`, but for a package a/b/__init__.py is `a.b`
        # ITSELF. Treating both the same drops every `from . import x` edge
        # inside an __init__, which is exactly where those imports live.
        base_parts = importer_module.split(".")
        drop = level - 1 if importer_is_pkg else level
        base = base_parts[: len(base_parts) - drop] if drop >= 0 else base_parts
        cand = ".".join([p for p in base if p] + ([mod] if mod else []))
    else:
        cand = mod

    if not cand:
        return None

    parts = cand.split(".")
    # longest-prefix match: the import may name a symbol inside a module
    for i in range(len(parts), 0, -1):
        probe = ".".join(parts[:i])
        if probe in index:
            return probe
    return None


# --------------------------------------------------------------------------
# entrypoints
# --------------------------------------------------------------------------
def base_name(rel: str) -> str:
    return rel.rsplit("/", 1)[-1]


def tracked_files(root: Path):
    """Paths git actually tracks, or None if this is not a git repo.

    An UNTRACKED file is a categorically different finding from a committed
    one: it is local scratch that vanishes on a fresh clone, not code the team
    is carrying. Reporting the two together conflates "someone left debris on
    their machine" with "the repo ships code nothing uses" -- on the first repo
    checked the split was exactly 12/12, and only the committed half was worth
    anyone's attention.
    """
    try:
        import subprocess
        r = subprocess.run(["git", "-C", str(root), "ls-files"],
                           capture_output=True, text=True, timeout=30)
        if r.returncode != 0:
            return None
        return {p.replace("/", os.sep) for p in r.stdout.splitlines() if p}
    except Exception:  # noqa: BLE001 - git absent or not a repo: no information
        return None


def manifest_entrypoints(root: Path):
    """What the repo's PROCESS MANIFESTS actually invoke.

    The TS phase learned this from package.json: convention locates candidate
    scripts, but only the project's own config says which are live -- 11 of 41
    scripts in one repo were wired, the other 30 were debris. Python has no
    single manifest, so consult all the usual ones.

    Returns (relpaths, dotted_modules, manifests_found). An empty
    manifests_found means "this repo declares nothing", which must be treated
    as no-information rather than as evidence of deadness.
    """
    names = ["Procfile", "Makefile", "makefile", "Dockerfile", "pyproject.toml",
             "setup.py", "setup.cfg", "railway.json", "railway.toml",
             "render.yaml", "fly.toml", "docker-compose.yml",
             "docker-compose.yaml", "package.json", "nixpacks.toml"]
    blobs, found = [], []
    for nm in names:
        p = root / nm
        if p.is_file():
            try:
                blobs.append(p.read_text("utf-8", errors="replace"))
                found.append(nm)
            except OSError:
                pass
    for wf in list((root / ".github" / "workflows").glob("*.y*ml")):
        try:
            blobs.append(wf.read_text("utf-8", errors="replace"))
            found.append(f".github/workflows/{wf.name}")
        except OSError:
            pass

    blob = "\n".join(blobs)
    paths, dotted = set(), set()

    # explicit file invocations: `python scripts/backfill.py`, or a bare path
    for m in re.finditer(r"[\w./\\-]+\.py\b", blob):
        paths.add(m.group(0).replace("\\", "/").lstrip("./"))
    # `python -m pkg.mod`
    for m in re.finditer(r"-m\s+([A-Za-z_][\w.]*)", blob):
        dotted.add(m.group(1))
    # ASGI/WSGI/celery targets and console_scripts: `app.main:app`
    for m in re.finditer(r"\b([A-Za-z_][\w.]*)\s*:\s*[A-Za-z_]\w*", blob):
        if "." in m.group(1):
            dotted.add(m.group(1))
    # `celery -A app.worker`, `alembic -c ...`
    for m in re.finditer(r"-A\s+([A-Za-z_][\w.]*)", blob):
        dotted.add(m.group(1))

    return paths, dotted, found


def alembic_version_dirs(root: Path):
    """Directories Alembic actually scans, read from alembic.ini.

    Content alone cannot decide this. A file with `revision = "..."` and an
    `upgrade()` IS a migration -- but an ARCHIVED one Alembic never loads is
    dead weight, and that is a finding, not an entrypoint. Only the config says
    which directories are live: `version_locations`, else the default
    `<script_location>/versions`.

    Returns None when there is no alembic.ini, meaning "fall back to content
    alone" -- the right call for a repo that wires Alembic programmatically.
    """
    inis = [p for p in (root / "alembic.ini", root / "setup.cfg") if p.is_file()]
    inis += [p for p in root.glob("*/alembic.ini") if p.is_file()]
    if not inis:
        return None

    dirs = set()
    for ini in inis:
        try:
            text = ini.read_text("utf-8", errors="replace")
        except OSError:
            continue
        base = ini.parent
        locs = re.search(r"^\s*version_locations\s*=\s*(.+)$", text, re.M)
        if locs:
            for part in re.split(r"[,\s]+", locs.group(1).strip()):
                if part:
                    dirs.add((base / part.replace("%(here)s", str(base))).resolve())
            continue
        script = re.search(r"^\s*script_location\s*=\s*(.+)$", text, re.M)
        if script:
            dirs.add((base / script.group(1).strip() / "versions").resolve())
    return dirs or None



def classify_entrypoints(nodes: dict, root: Path):
    """Mark modules reachable by something other than a static import.

    This is the step that decides whether `dead` is meaningful. Every class
    here is a real invocation path that produces NO import edge, so omitting
    one manufactures false orphans in bulk.
    """
    live_version_dirs = alembic_version_dirs(root)
    manifest_paths, manifest_dotted, manifests_found = manifest_entrypoints(root)

    for mod, n in nodes.items():
        rel = n["path"].replace("\\", "/")
        reasons = []
        n["runnable"] = False

        # Alembic migrations -- invoked by alembic, never imported.
        #
        # Needs BOTH signals. Path alone fitted one repo's layout and reported
        # every migration in another (`mcp_server/db/versions/`) as an orphan.
        # Content alone then swung the other way and blessed a repo's ARCHIVED
        # migrations as live entrypoints, hiding 7 true findings. So: content
        # says "this is a migration", the alembic config says "and this one is
        # actually loaded".
        src = n.get("_src", "")
        is_migration = (
            re.search(r"^revision(?::\s*str)?\s*=\s*['\"]", src, re.M)
            and re.search(r"^def (upgrade|downgrade)\s*\(", src, re.M)
        )
        if is_migration:
            if live_version_dirs is None:
                reasons.append("alembic-migration")     # no config to consult
            elif any(d in (root / n["path"]).resolve().parents
                     for d in live_version_dirs):
                reasons.append("alembic-migration")
            # else: a migration outside version_locations -- Alembic never
            # loads it, so it stays a dead-code candidate.
        if base_name(rel) == "env.py" and re.search(
                r"\bfrom alembic import\b|\balembic\.context\b|\bcontext\.configure\s*\(",
                src):
            reasons.append("alembic-env")

        # pytest -- collected by the runner, not imported
        base = os.path.basename(rel)
        if base == "conftest.py":
            reasons.append("pytest-conftest")
        elif base.startswith("test_") or base.endswith("_test.py"):
            reasons.append("pytest-test")
        elif "/tests/" in f"/{rel}":
            reasons.append("in-tests-tree")

        # package initializers -- imported implicitly by the package machinery
        if base == "__init__.py":
            reasons.append("package-init")

        # module-level app construction -- the ASGI target itself
        if "FastAPI(" in src:
            reasons.append("fastapi-app")

        # Declared in a process manifest: Procfile, Makefile, pyproject
        # [project.scripts], CI workflow, Dockerfile CMD. This is the only
        # evidence that a script is actually WIRED into something.
        if rel in manifest_paths or mod in manifest_dotted or \
                any(mod.startswith(d + ".") for d in manifest_dotted):
            reasons.append("manifest-declared")

        # Runnable but NOT wired. `if __name__ == "__main__"` and living under
        # scripts/ used to confer reachability outright, which is the same
        # blanket blessing that hid 30 unwired scripts on the TS side. A script
        # someone can run by hand is not dead -- but it is not referenced
        # either, and conflating those two hides both. Record it as a distinct
        # state so `dead` can report it as its own category.
        runnable = (
            ("__main__" in src and "if __name__" in src)
            or rel.startswith("scripts/") or "/scripts/" in f"/{rel}"
        )
        n["runnable"] = bool(runnable)
        # With no manifest at all, the repo declares nothing and we have no
        # information -- fall back to the old permissive behaviour rather than
        # inventing findings out of an absence of config.
        if runnable and not manifests_found:
            reasons.append("script-no-manifest")

        if reasons:
            n["entrypoint"] = reasons


# --------------------------------------------------------------------------
# graph build
# --------------------------------------------------------------------------
def build(root: Path) -> dict:
    root = root.resolve()
    files = sorted(walk_py(root))

    nodes: dict[str, dict] = {}
    for f in files:
        try:
            mod = module_name_for(f, root)
        except ValueError:
            continue
        src = f.read_bytes().decode("utf-8", errors="replace")
        nodes[mod] = {
            "module": mod,
            "path": str(f.relative_to(root)),
            "is_pkg": f.name == "__init__.py",
            "loc": src.count("\n") + 1,
            "entrypoint": [],
            "_src": src,
        }

    index = {m: n["path"] for m, n in nodes.items()}

    edges = set()                    # (importer, imported) -- deduped
    unresolved = defaultdict(list)   # importer -> [raw import]
    parse_errors = {}

    opaque_prefixes = []
    opaque_sites = []      # (path:line, bounded-prefix or "")
    for mod, n in nodes.items():
        imports, err, opaque = parse_imports(root / n["path"])
        for pref, lineno in opaque:
            if pref == SELF_PREFIX:
                pref = mod + "."      # `__name__` is this module's own name
            opaque_prefixes.append(pref)
            opaque_sites.append([f"{n['path']}:{lineno}", pref])
        if err:
            parse_errors[mod] = err
            continue
        for raw, level in imports:
            target = resolve(raw, level, mod, index, n["is_pkg"])
            if target and target != mod:
                edges.add((mod, target))
            elif level or raw.split(".")[0] in {p.split(".")[0] for p in index}:
                # looked internal but did not resolve -- RECORD it, never drop
                unresolved[mod].append(f"{'.' * level}{raw}")

    classify_entrypoints(nodes, root)

    # Dynamic machinery that we could NOT resolve to a literal target.
    # A literal import_module("a.b") already became a real edge above, so it
    # must not also count as an unknown -- that double-counting is what made
    # every tier collapse to `suspect` in v1.
    dynamic_hits = defaultdict(list)
    for mod, n in nodes.items():
        for rx, label in DYNAMIC_PATTERNS:
            if not rx.search(n["_src"]):
                continue
            _, _, opaque = parse_imports(root / n["path"])
            if opaque:
                dynamic_hits[label].append(mod)

    # reachability BFS from entrypoints
    fwd = defaultdict(set)
    rev = defaultdict(set)
    for a, b in edges:
        fwd[a].add(b)
        rev[b].add(a)

    roots = [m for m, n in nodes.items() if n["entrypoint"]]
    seen = set(roots)
    q = deque(roots)
    while q:
        cur = q.popleft()
        for nxt in fwd.get(cur, ()):
            if nxt not in seen:
                seen.add(nxt)
                q.append(nxt)

    for m, n in nodes.items():
        n["reachable"] = m in seen
        n["importers"] = sorted(rev.get(m, ()))
        n["imports"] = sorted(fwd.get(m, ()))
        n.pop("_src", None)

    return {
        "root": str(root),
        "language": "python",
        "counts": {
            "modules": len(nodes),
            "edges": len(edges),
            "entrypoints": len(roots),
            "reachable": len(seen),
            "unreached": len(nodes) - len(seen),
            "unresolved_imports": sum(len(v) for v in unresolved.values()),
            "parse_errors": len(parse_errors),
        },
        "nodes": nodes,
        "edges": [list(e) for e in sorted(set(edges))],
        "unresolved": {k: v for k, v in sorted(unresolved.items())},
        "parse_errors": parse_errors,
        "dynamic_patterns": {k: sorted(v) for k, v in sorted(dynamic_hits.items())},
        "opaque_prefixes": sorted({p for p in opaque_prefixes if p}),
        # True only when some dynamic call's reach could NOT be bounded (a bare
        # variable target). That one really can hit anything, so it is the only
        # case that justifies casting doubt on the entire repo.
        "opaque_unbounded": any(p == "" for p in opaque_prefixes),
        # Every opaque call site, so an unbounded one names itself instead of
        # silently capping the whole repo at `suspect`.
        "opaque_sites": sorted(opaque_sites),
        "manifests_found": sorted(manifest_entrypoints(root)[2]),
    }


# --------------------------------------------------------------------------
# dead-code tiering
# --------------------------------------------------------------------------
def dead(graph: dict, root: Path):
    """Tier every unreached module by how confident we are it is truly dead.

    certain : unreached, zero importers, and its bare module name appears
              NOWHERE else in the repo's text (not in code, config, or docs)
    likely  : unreached, zero importers, name appears somewhere but not as an
              import we could resolve
    suspect : unreached but the repo uses dynamic-import machinery, so static
              analysis cannot prove anything
    """
    nodes = graph["nodes"]

    # Downgrading for dynamic imports must be SCOPED to what those imports can
    # actually reach. A single `iter_modules(mcp_server.tools.__path__)` used to
    # push every module in the repo to `suspect` -- the tool then said nothing
    # at all, which is the same silence as reporting no findings but disguised
    # as caution. A bounded prefix casts doubt on that subtree only.
    opaque_prefixes = graph.get("opaque_prefixes", [])
    opaque_unbounded = graph.get("opaque_unbounded", False)

    def dyn_shadowed(mod: str) -> str | None:
        if opaque_unbounded:
            return "repo has a dynamic import whose target could not be bounded"
        for p in opaque_prefixes:
            if mod.startswith(p):
                return f"reachable via dynamic import under `{p}*`"
        return None

    tracked = tracked_files(root)
    unreached = [m for m, n in nodes.items() if not n["reachable"]]

    # Build one text corpus scan for name occurrences (cheap, one pass).
    corpus = {}
    for m, n in nodes.items():
        p = root / n["path"]
        try:
            corpus[m] = p.read_bytes().decode("utf-8", errors="replace")
        except Exception:
            corpus[m] = ""

    # also scan non-python text files -- configs reference modules by string
    extra_text = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if fn.endswith((".toml", ".ini", ".cfg", ".yaml", ".yml", ".json",
                            ".txt", ".md", ".env", ".sh", "Dockerfile")):
                try:
                    extra_text.append((root / dirpath / fn).read_bytes()
                                      .decode("utf-8", errors="replace"))
                except Exception:
                    pass
    extra_blob = "\n".join(extra_text)

    results = []
    for m in sorted(unreached):
        n = nodes[m]
        leaf = m.split(".")[-1]
        # occurrences of the leaf name outside its own file
        hits = 0
        for other, text in corpus.items():
            if other == m:
                continue
            if re.search(rf"\b{re.escape(leaf)}\b", text):
                hits += 1
        cfg_hit = bool(re.search(rf"\b{re.escape(leaf)}\b", extra_blob))

        shadow = dyn_shadowed(m)
        if shadow:
            tier = "suspect"
            why = shadow
        elif n.get("runnable"):
            tier = "unwired"
            why = ("runnable by hand, but declared in no process manifest "
                   f"({', '.join(graph.get('manifests_found') or ['none found'])})")
        elif n["importers"]:
            tier = "suspect"
            why = "has importers but is not reachable from any entrypoint"
        elif hits == 0 and not cfg_hit:
            tier = "certain"
            why = "no importers, no entrypoint, zero textual references repo-wide"
        else:
            tier = "likely"
            why = f"no import edge, but name appears in {hits} module(s)" + \
                  (" and in config/docs" if cfg_hit else "")

        results.append({
            "module": m, "path": n["path"], "loc": n["loc"],
            "tier": tier, "why": why, "text_refs": hits, "config_ref": cfg_hit,
            "tracked": (None if tracked is None else n["path"] in tracked),
        })
    return results


# --------------------------------------------------------------------------
# cli
# --------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("command", choices=["build", "dead", "touch", "stats"])
    ap.add_argument("target", nargs="?")
    ap.add_argument("--root", required=True)
    ap.add_argument("--out")
    ap.add_argument("--tier",
                    choices=["certain", "unwired", "likely", "suspect"])
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--lang", choices=["py", "ts", "rs", "auto"], default="auto",
                    help="parser to use; `auto` picks by what the repo contains")
    ap.add_argument("--ignore-dynamic", action="append", default=[],
                    metavar="PATH",
                    help="path substring of an opaque dynamic import a human "
                         "has already adjudicated; stops it capping tiers. "
                         "Repeatable.")
    args = ap.parse_args()

    root = Path(args.root).resolve()
    if not root.is_dir():
        print(f"ERROR: not a directory: {root}", file=sys.stderr)
        return 2

    # Pick the parser. A repo with several languages gets ONE of them and is
    # told plainly which trees went un-analysed; a repo with none gets an
    # explicit refusal, never a silent clean bill of health.
    import codegraph_ts
    import codegraph_rs

    LANGS = {
        "py": ("Python", walk_py, build, dead),
        "ts": ("TS/JS", codegraph_ts.walk_src, codegraph_ts.build,
               codegraph_ts.dead),
        "rs": ("Rust", codegraph_rs.walk_src, codegraph_rs.build,
               codegraph_rs.dead),
    }

    lang = args.lang
    if lang == "auto":
        # Declared priority, not a heuristic: a repo's answer must not change
        # because someone added a file. Whatever is picked, every other
        # language present is named below with the command that graphs it.
        present = [k for k in ("py", "ts", "rs")
                   if next(LANGS[k][1](root), None) is not None]
        lang = present[0] if present else "py"
        for other in present[1:]:
            print(f"NOTE: repo also contains {LANGS[other][0]} source, NOT "
                  f"analysed by this run.", file=sys.stderr)
            print(f"      re-run with --lang {other} to graph it.",
                  file=sys.stderr)

    lang_label, _walk, build_fn, dead_fn = LANGS[lang]
    g = build_fn(root)

    # Drop adjudicated call sites BEFORE tiering, so a human ruling actually
    # buys back real answers instead of just annotating the same silence.
    if args.ignore_dynamic:
        keep = [[s, pref] for s, pref in g.get("opaque_sites", [])
                if not any(ig.replace("\\", "/") in s.replace("\\", "/")
                           for ig in args.ignore_dynamic)]
        dropped = len(g.get("opaque_sites", [])) - len(keep)
        g["opaque_sites"] = keep
        g["opaque_unbounded"] = any(not pref for _s, pref in keep)
        g["opaque_prefixes"] = sorted({pref for _s, pref in keep if pref})
        print(f"(ignoring {dropped} adjudicated dynamic call site(s))")

    # An empty graph must never read as a clean bill of health. Pointing the
    # Python parser at a Rust repo yields "0 UNREACHED" -- which looks like
    # "nothing is dead" and is the single most misleading thing this tool could
    # say. Name the reason instead.
    if not g["nodes"]:
        print(f"NO {lang_label.upper()} MODULES FOUND under {root}",
              file=sys.stderr)
        print(f"This is NOT a finding of 'no dead code' -- this run parsed "
              f"{lang_label} only.", file=sys.stderr)
        others = [f"--lang {k}" for k in LANGS if k != lang]
        print(f"If this repo is another language, try: {', '.join(others)}",
              file=sys.stderr)
        return 3

    # Stamp the tier onto every node so the emitted JSON is self-describing and
    # validate_dead.py can consume it as data without importing this module.
    for d in dead_fn(g, root):
        g["nodes"][d["module"]]["dead_tier"] = d["tier"]

    if args.command in ("build", "stats"):
        c = g["counts"]
        print(f"root:               {g['root']}")
        print(f"modules:            {c['modules']}")
        print(f"import edges:       {c['edges']}")
        print(f"entrypoints:        {c['entrypoints']}")
        print(f"reachable:          {c['reachable']}")
        print(f"UNREACHED:          {c['unreached']}")
        print(f"unresolved imports: {c['unresolved_imports']}  "
              f"(recorded, not dropped)")
        print(f"parse errors:       {c['parse_errors']}")
        if g.get("dynamic_patterns"):
            print("\ndynamic-import machinery (downgrades only the subtrees it can reach):")
            for k, v in g["dynamic_patterns"].items():
                print(f"  {k:<18} {len(v)} module(s)")
        if args.command == "build" and args.out:
            Path(args.out).write_text(json.dumps(g, indent=2), encoding="utf-8")
            print(f"\nwrote {args.out}")
        return 0

    if args.command == "dead":
        rows = dead_fn(g, root)
        if args.tier:
            rows = [r for r in rows if r["tier"] == args.tier]
        if args.json:
            print(json.dumps(rows, indent=2))
            return 0
        by = defaultdict(list)
        for r in rows:
            by[r["tier"]].append(r)
        for tier in ("certain", "unwired", "likely", "suspect"):
            if tier not in by:
                continue
            print(f"\n=== {tier.upper()}  ({len(by[tier])}) ===")
            if by[tier]:
                print(f"    {by[tier][0]['why']}")
            # Untracked files are local scratch that vanishes on a fresh
            # clone -- a different problem from code the repo actually ships.
            # Listing them together makes a tidy repo look messy.
            shown = by[tier][:60]
            committed = [r for r in shown if r.get("tracked") is not False]
            scratch = [r for r in shown if r.get("tracked") is False]
            for r in committed:
                print(f"  {r['loc']:>5} loc  {r['path']}")
            if scratch:
                print(f"  -- not in git ({len(scratch)}): local scratch, "
                      f"vanishes on a fresh clone --")
                for r in scratch:
                    print(f"  {r['loc']:>5} loc  {r['path']}")
            if len(by[tier]) > 60:
                print(f"  ... and {len(by[tier]) - 60} more")

        # A capped report must name what capped it. One unbounded call can push
        # an entire repo to `suspect`, and a reader with no call site to look at
        # can only conclude the tool is useless. Point at the line; bounding or
        # allowlisting it is then a two-minute job.
        if g.get("opaque_unbounded"):
            blind = [s for s, pref in g.get("opaque_sites", []) if not pref]
            print("\n--- why tiers are capped at `suspect` ---")
            print("    These dynamic imports have targets that cannot be")
            print("    resolved statically, so any module might be reached:")
            for site in blind[:10]:
                print(f"      {site}")
            if len(blind) > 10:
                print(f"      ... and {len(blind) - 10} more")
            print("    Bound them (literal or f-string prefix), or re-run with")
            print("    --ignore-dynamic <path> once you have ruled on each.")
        return 0

    if args.command == "touch":
        if not args.target:
            print("ERROR: touch needs a path", file=sys.stderr)
            return 2
        t = args.target.replace("\\", "/")
        hit = [m for m, n in g["nodes"].items()
               if n["path"].replace("\\", "/").endswith(t)]
        if not hit:
            print(f"no module matching {t}")
            return 1
        for m in hit:
            n = g["nodes"][m]
            print(f"\n=== {n['path']} ===")
            print(f"  reachable:  {n['reachable']}  entrypoint: {n['entrypoint'] or '-'}")
            print(f"  imported by ({len(n['importers'])}):")
            for i in n["importers"][:25]:
                print(f"    {g['nodes'][i]['path']}")
            print(f"  imports ({len(n['imports'])}):")
            for i in n["imports"][:25]:
                print(f"    {g['nodes'][i]['path']}")
        return 0

    return 0


if __name__ == "__main__":
    sys.exit(main())
