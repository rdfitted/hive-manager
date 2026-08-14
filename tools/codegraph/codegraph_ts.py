#!/usr/bin/env python3
# Vendored from C:/Users/RDuff/.claude/tools/codegraph/codegraph_ts.py on 2026-08-14; source SHA-256 474860D54F1DB3B5DE2054B0889842A0580102CABD5559C926863C397F9DA7A1; upstream commit unavailable (local source is not a Git worktree).
"""codegraph TS/TSX parser -- phase 2. READ-ONLY, zero dependencies.

Same contract as the Python phase: deterministic, never silently drop an edge,
report-never-delete, and only `certain` is ever suggested for deletion.

WHY REGEX AND NOT A REAL PARSER
There is no TypeScript AST in the stdlib and the tool takes no dependencies.
Import and export statements are highly regular, so a scanner is workable --
but ONLY after comments, strings and template literals are stripped, because
otherwise `// import './old'` and `"import x from 'y'"` inside a test fixture
both register as edges. That is the same lesson the Python oracle learned when
`alembic upgrade head` in a docstring matched every migration's upgrade().
Stripping is therefore not a nicety here, it is the correctness step.

RESOLUTION RULES THIS REPO SHAPE DEMANDS
  * Extensionless specifiers: './x' may be x.ts, x.tsx, x.js, x.jsx, x/index.ts
    ... Trying only one order manufactures orphans in bulk.
  * `lazy(() => import('./Home'))` -- a dynamic import with a LITERAL argument
    is statically resolvable and must become a real edge. React route-level
    code splitting means most page components are reached ONLY this way; miss
    it and every page in the app reads as dead.
  * `export * from './x'` and `export { a } from './x'` are edges too. Barrel
    files are pure re-export, so treating them as leaves strands whole trees.
  * tsconfig `paths` aliases, when present.
"""
from __future__ import annotations

import json
import os
import re
from pathlib import Path

SKIP_DIRS = {
    ".git", "node_modules", "dist", "build", ".next", ".turbo", "coverage",
    ".venv", "venv", "out", ".cache", ".parcel-cache", ".vite", "storybook-static",
    ".idea", ".vscode", "logs", "test-results", "playwright-report",
    # Agent worktrees are near-complete repo COPIES whose clones import each
    # other -- they mask true findings, they do not merely inflate counts.
    "worktrees", ".claude", ".hive-manager", ".worktrees",
    # Agent session artifacts: transcripts, plans and task files that name
    # modules in PROSE. They are not code and not source of truth, but their
    # mentions are enough to demote a true finding from `certain` to `likely`.
    ".hive", ".swarm", ".agent-sessions",
}

EXTS = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]

# Imported, but never a module edge: stylesheets and static assets carry no
# dependency meaning for reachability. Recording them as "unresolved internal"
# would bury the specifiers that genuinely failed to resolve.
ASSET_EXTS = (".css", ".scss", ".sass", ".less", ".styl", ".svg", ".png",
              ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".ico", ".json",
              ".graphql", ".gql", ".md", ".txt", ".wasm", ".woff", ".woff2")
# Order matters: a specifier './x' prefers x.ts over x/index.ts, and .ts over
# .js, matching TypeScript's own resolution.
RESOLVE_SUFFIXES = (
    [e for e in EXTS]
    + [f"/index{e}" for e in EXTS]
)

# --- statement scanners, run on COMMENT/STRING-STRIPPED source -------------
RX_IMPORT_FROM = re.compile(r"""\bimport\b[^;'"]*?\bfrom\s*['"]([^'"]+)['"]""")
RX_IMPORT_BARE = re.compile(r"""\bimport\s*['"]([^'"]+)['"]""")     # side-effect
RX_EXPORT_FROM = re.compile(r"""\bexport\b[^;'"]*?\bfrom\s*['"]([^'"]+)['"]""")
RX_DYNAMIC = re.compile(r"""\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)""")
RX_REQUIRE = re.compile(r"""\brequire\s*\(\s*['"]([^'"]+)['"]\s*\)""")
# A dynamic import whose target is a template literal: import(`./pages/${n}`).
# The constant lead bounds its reach the same way an f-string lead does.
RX_DYNAMIC_TPL = re.compile(r"""\bimport\s*\(\s*`([^`$]*)\$\{""")
RX_DYNAMIC_OPAQUE = re.compile(r"""\bimport\s*\(\s*(?!['"`])[A-Za-z_$]""")


def strip_code(src: str) -> str:
    """Remove comments, string and template literals, keeping line structure.

    Replaces content with spaces rather than deleting it so that line numbers
    survive for call-site reporting. Import specifiers are re-extracted from
    the ORIGINAL source; this stripped copy exists only to decide WHERE a
    statement legitimately occurs.
    """
    out = []
    i, n = 0, len(src)
    state = None   # None | "line" | "block" | "'" | '"' | "`"
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if state is None:
            if c == "/" and nxt == "/":
                state, i = "line", i + 2
                out.append("  ")
                continue
            if c == "/" and nxt == "*":
                state, i = "block", i + 2
                out.append("  ")
                continue
            if c in "'\"`":
                state = c
                out.append(c)
                i += 1
                continue
            out.append(c)
            i += 1
        elif state == "line":
            if c == "\n":
                state = None
                out.append(c)
            else:
                out.append(" ")
            i += 1
        elif state == "block":
            if c == "*" and nxt == "/":
                state, i = None, i + 2
                out.append("  ")
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
        else:  # inside a string/template literal
            if c == "\\":
                out.append("  ")
                i += 2
                continue
            if c == state:
                state = None
                out.append(c)
            else:
                out.append("\n" if c == "\n" else " ")
            i += 1
    return "".join(out)


def specifiers(src: str):
    """(specifiers, template_prefixes, opaque_count) from one file.

    Runs the scanners on a copy whose comments are blanked but whose string
    CONTENT is preserved, so specifiers stay readable while `// import 'x'`
    does not register.
    """
    scan = strip_comments_only(src)
    specs = []
    for rx in (RX_IMPORT_FROM, RX_EXPORT_FROM, RX_DYNAMIC, RX_REQUIRE,
               RX_IMPORT_BARE):
        specs.extend(rx.findall(scan))
    tpl = [m for m in RX_DYNAMIC_TPL.findall(scan) if m]
    opaque = len(RX_DYNAMIC_OPAQUE.findall(scan))
    return specs, tpl, opaque


def strip_comments_only(src: str) -> str:
    """Blank comments but keep string contents (specifiers live in strings)."""
    out = []
    i, n = 0, len(src)
    state = None
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if state is None:
            if c == "/" and nxt == "/":
                state, i = "line", i + 2
                out.append("  ")
                continue
            if c == "/" and nxt == "*":
                state, i = "block", i + 2
                out.append("  ")
                continue
            if c in "'\"`":
                state = c
            out.append(c)
            i += 1
        elif state == "line":
            if c == "\n":
                state = None
            out.append(c if c == "\n" else " ")
            i += 1
        elif state == "block":
            if c == "*" and nxt == "/":
                state, i = None, i + 2
                out.append("  ")
                continue
            out.append(c if c == "\n" else " ")
            i += 1
        else:
            if c == "\\":
                out.append(src[i:i + 2])
                i += 2
                continue
            if c == state:
                state = None
            out.append(c)
            i += 1
    return "".join(out)


# --------------------------------------------------------------------------
def walk_src(root: Path):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if any(fn.endswith(e) for e in EXTS):
                yield Path(dirpath) / fn


def tsconfig_paths(root: Path):
    """{alias-prefix: [target-prefix, ...]} from tsconfig `paths`, if any."""
    aliases = {}
    for name in ("tsconfig.json", "tsconfig.app.json", "jsconfig.json"):
        p = root / name
        if not p.is_file():
            continue
        try:
            # tsconfig allows comments and trailing commas; JSON does not.
            raw = strip_comments_only(p.read_text("utf-8", errors="replace"))
            raw = re.sub(r",(\s*[}\]])", r"\1", raw)
            cfg = json.loads(raw)
        except (OSError, ValueError):
            continue
        co = cfg.get("compilerOptions", {})
        base = co.get("baseUrl", ".")
        for alias, targets in (co.get("paths") or {}).items():
            aliases[alias.rstrip("*")] = [
                str((root / base / t.rstrip("*")).resolve()) for t in targets
            ]
    return aliases


def resolve_spec(spec: str, importer: Path, root: Path, index: set,
                 aliases: dict):
    """Resolve a specifier to an internal file, or None if external/unknown.

    Returns (resolved_relpath | None, looked_internal: bool). The second value
    is what keeps an unresolvable-but-internal specifier from being silently
    dropped -- it gets RECORDED instead.
    """
    if spec.lower().endswith(ASSET_EXTS):
        return None, False        # asset, not a module -- external by nature

    cands = []
    if spec.startswith("."):
        base = (importer.parent / spec).resolve()
        cands.append(base)
        internal = True
    else:
        internal = False
        for prefix, targets in aliases.items():
            if prefix and spec.startswith(prefix):
                rest = spec[len(prefix):]
                for t in targets:
                    cands.append(Path(t) / rest)
                internal = True
                break
        if not cands:
            # bare specifier -> node_modules package, genuinely external
            return None, False

    for base in cands:
        # exact file first, then extension and index resolution
        if str(base) in index and base.is_file():
            return base, internal
        for suf in RESOLVE_SUFFIXES:
            probe = Path(str(base) + suf)
            if str(probe) in index:
                return probe, internal
        # TypeScript ESM: the SPECIFIER carries the emitted `.js` extension
        # while the file on disk is `.ts`/`.tsx`. Not rewriting this stranded
        # 2270 real edges on the first repo it was tried against -- imports
        # like '../../types/user.js' pointing at user.ts.
        m = re.search(r"\.(js|jsx|mjs|cjs)$", str(base))
        if m:
            stem = str(base)[: m.start()]
            for suf in RESOLVE_SUFFIXES:
                probe = Path(stem + suf)
                if str(probe) in index:
                    return probe, internal
    return None, internal


def build(root: Path) -> dict:
    root = root.resolve()
    files = sorted(walk_src(root))
    index = {str(f) for f in files}
    aliases = tsconfig_paths(root)

    nodes = {}
    for f in files:
        try:
            src = f.read_text("utf-8", errors="replace")
        except OSError:
            continue
        nodes[str(f)] = {
            "path": str(f.relative_to(root)),
            "loc": src.count("\n") + 1,
            "entrypoint": [],
            "_src": src,
        }

    edges = set()
    unresolved = {}
    tpl_prefixes, opaque_total, opaque_sites = [], 0, []

    for key, n in nodes.items():
        f = Path(key)
        specs, tpls, opaque = specifiers(n["_src"])
        for pref in tpls:
            base = (f.parent / pref).resolve()
            tpl_prefixes.append(str(base))
        if opaque:
            opaque_total += opaque
            opaque_sites.append(n["path"])
        for spec in specs:
            target, looked_internal = resolve_spec(spec, f, root, index, aliases)
            if target is not None and str(target) != key:
                edges.add((key, str(target)))
            elif looked_internal:
                unresolved.setdefault(n["path"], []).append(spec)

    classify_entrypoints(nodes, root)

    fwd, rev = {}, {}
    for a, b in edges:
        fwd.setdefault(a, set()).add(b)
        rev.setdefault(b, set()).add(a)

    roots = [k for k, n in nodes.items() if n["entrypoint"]]
    seen, queue = set(roots), list(roots)
    while queue:
        cur = queue.pop()
        for nxt in fwd.get(cur, ()):
            if nxt not in seen:
                seen.add(nxt)
                queue.append(nxt)

    out_nodes = {}
    for k, n in nodes.items():
        n["reachable"] = k in seen
        n["importers"] = sorted(nodes[i]["path"] for i in rev.get(k, ()))
        n["imports"] = sorted(nodes[i]["path"] for i in fwd.get(k, ()))
        n.pop("_src", None)
        out_nodes[n["path"]] = n

    return {
        "root": str(root),
        "language": "typescript",
        "counts": {
            "modules": len(nodes),
            "edges": len(edges),
            "entrypoints": len(roots),
            "reachable": len(seen),
            "unreached": len(nodes) - len(seen),
            "unresolved_imports": sum(len(v) for v in unresolved.values()),
            "parse_errors": 0,
        },
        "nodes": out_nodes,
        "edges": sorted([nodes[a]["path"], nodes[b]["path"]] for a, b in edges),
        "unresolved": unresolved,
        "parse_errors": {},
        "aliases": {k: v for k, v in aliases.items()},
        "opaque_prefixes": sorted({p for p in tpl_prefixes}),
        "opaque_unbounded": opaque_total > 0,
        "opaque_sites": sorted([[s, ""] for s in set(opaque_sites)]),
    }


def classify_entrypoints(nodes: dict, root: Path):
    """Mark files reached by something other than an import.

    Omitting a class here manufactures false orphans in bulk, so each entry is
    a real invocation path that produces no import edge.
    """
    html_entries = set()
    for html in list(root.glob("*.html")) + list(root.glob("*/*.html")):
        try:
            text = html.read_text("utf-8", errors="replace")
        except OSError:
            continue
        for m in re.finditer(r"""<script[^>]*\bsrc\s*=\s*['"]([^'"]+)['"]""", text):
            src = m.group(1).lstrip("/")
            html_entries.add(str((html.parent / src).resolve()))

    # package.json `scripts` is the authority on which one-off scripts are
    # actually wired. Blanket-blessing everything under scripts/ (as the Python
    # phase does) hides the unreferenced ones, and on the first repo tried only
    # 11 of 41 were wired -- the other 30 are exactly the finding worth having.
    # Same shape as the Alembic rule: convention locates candidates, the
    # project's own config decides which are live.
    npm_referenced = set()
    pkg = root / "package.json"
    if pkg.is_file():
        try:
            data = json.loads(pkg.read_text("utf-8", errors="replace"))
            blob = " ".join(str(v) for v in (data.get("scripts") or {}).values())
            blob += " " + str(data.get("main", "")) + " " + str(data.get("bin", ""))
            for m in re.finditer(r"[\w./\\-]+\.(?:ts|tsx|js|jsx|mjs|cjs)\b", blob):
                npm_referenced.add(str((root / m.group(0)).resolve()))
            # ts-node/tsx invocations sometimes omit the extension
            for m in re.finditer(r"(?:tsx|ts-node[\w/]*)\s+([\w./\\-]+)", blob):
                cand = (root / m.group(1)).resolve()
                npm_referenced.add(str(cand))
                for e in EXTS:
                    npm_referenced.add(str(cand) + e)
        except (OSError, ValueError):
            pass

    for key, n in nodes.items():
        rel = n["path"].replace("\\", "/")
        base = rel.rsplit("/", 1)[-1]
        reasons = []

        if key in html_entries:
            reasons.append("html-script-entry")
        if key in npm_referenced:
            reasons.append("npm-script")
        # Runnable by hand (`npx tsx src/scripts/x.ts`) but wired to nothing.
        # Not dead -- it executes -- but not referenced either. Conflating the
        # two hides both, so record it as its own state.
        n["runnable"] = bool(
            re.search(r"(^|/)scripts?/", rel) and key not in npm_referenced
        )
        # Vite/Vitest/Tailwind/PostCSS/ESLint configs are loaded by their tools
        if re.match(r"^[\w.-]*\.?(config|rc)\.(ts|js|mjs|cjs|tsx|jsx)$", base):
            reasons.append("tooling-config")
        if re.search(r"\.(test|spec)\.(ts|tsx|js|jsx)$", base):
            reasons.append("test")
        elif "/__tests__/" in f"/{rel}" or "/tests/" in f"/{rel}" or \
                "/e2e/" in f"/{rel}":
            reasons.append("in-tests-tree")
        if base.endswith(".d.ts"):
            reasons.append("ambient-types")     # referenced by the compiler
        if re.search(r"(^|/)(setupTests|setup|vitest\.setup|jest\.setup)\.",
                     rel):
            reasons.append("test-setup")
        # Firebase / serverless function roots are invoked by the platform
        if re.search(r"(^|/)functions/(src/)?index\.(ts|js)$", rel):
            reasons.append("cloud-function")
        if re.search(r"(^|/)(service-worker|sw|firebase-messaging-sw)\.",
                     rel):
            reasons.append("service-worker")
        # Vite treats src/main.* as the app root when index.html points at it
        if re.match(r"^(src/)?(main|index)\.(ts|tsx|js|jsx)$", rel):
            reasons.append("app-root")

        n["entrypoint"] = reasons


# --------------------------------------------------------------------------
# dead-code tiering  (mirrors the Python phase; only leaf-name derivation and
# the corpus extensions differ)
# --------------------------------------------------------------------------
TEXT_SCAN_EXT = (".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json",
                 ".html", ".md", ".yml", ".yaml", ".toml", ".css", ".scss")


def dead(graph: dict, root: Path):
    from codegraph import tracked_files
    tracked = tracked_files(root)
    nodes = graph["nodes"]
    opaque_prefixes = graph.get("opaque_prefixes", [])
    opaque_unbounded = graph.get("opaque_unbounded", False)

    corpus = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if fn.lower().endswith(TEXT_SCAN_EXT):
                p = Path(dirpath) / fn
                try:
                    corpus.append((str(p.relative_to(root)),
                                   p.read_text("utf-8", errors="replace")))
                except OSError:
                    pass

    results = []
    for rel in sorted(m for m, n in nodes.items() if not n["reachable"]):
        n = nodes[rel]
        stem = Path(rel).name
        for e in EXTS:
            if stem.endswith(e):
                stem = stem[: -len(e)]
                break

        rx = re.compile(rf"\b{re.escape(stem)}\b")
        hits = sum(1 for other, text in corpus
                   if other != rel and rx.search(text))

        abs_mod = str((root / rel).resolve())
        shadow = None
        if opaque_unbounded:
            shadow = "repo has a dynamic import whose target could not be bounded"
        else:
            for p in opaque_prefixes:
                if abs_mod.startswith(p):
                    shadow = f"reachable via dynamic import under `{p}*`"
                    break

        if shadow:
            tier, why = "suspect", shadow
        elif n.get("runnable"):
            tier, why = "unwired", "runnable by hand, but wired to no package.json script"
        elif n["importers"]:
            tier, why = "suspect", "has importers but is not reachable from any entrypoint"
        elif hits == 0:
            tier, why = "certain", "no importers, no entrypoint, zero textual references repo-wide"
        else:
            tier, why = "likely", f"no import edge, but name appears in {hits} file(s)"

        results.append({"module": rel, "path": rel, "loc": n["loc"],
                        "tier": tier, "why": why, "text_refs": hits,
                        "tracked": (None if tracked is None else rel in tracked)})
    return results
