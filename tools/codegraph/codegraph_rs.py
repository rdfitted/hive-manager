#!/usr/bin/env python3
# Vendored from C:/Users/RDuff/.claude/tools/codegraph/codegraph_rs.py on 2026-08-14; source SHA-256 7D6F6ED1D8171EF382313AE284CEF692AA2AE5EB654833173E3A11064B942460; upstream commit unavailable (local source is not a Git worktree).
"""codegraph Rust parser -- phase 3. READ-ONLY, zero dependencies.

Same contract as the Python and TS phases: deterministic, never silently drop
an edge, report-never-delete, and only `certain` is ever suggested for
deletion.

WHY RUST IS DIFFERENT FROM BOTH PREVIOUS PHASES
A .rs file is compiled ONLY if a `mod` declaration mounts it into a crate,
transitively from a crate root (src/main.rs, src/lib.rs, src/bin/*, tests/*,
benches/*, examples/*, build.rs, or an explicit `path =` target in
Cargo.toml). `use` statements never bring a file in -- they only reference
modules that are already mounted. So the load-bearing edges here are MOUNT
edges, and an unmounted file is not "probably dead", it is provably never
compiled. rustc emits no warning for orphaned files, which is exactly why this
finding class is worth having.

SCANNER, NOT A PARSER -- same reasoning as the TS phase: no Rust AST in the
stdlib, no dependencies taken. `mod`/`use` items are highly regular, but ONLY
after comments and strings are stripped. Rust adds three stripping hazards the
TS scanner never faced: block comments NEST (/* /* */ */), raw strings carry
arbitrary hash fences (r#"..."#), and a bare apostrophe is usually a LIFETIME
('a) rather than a char literal -- treating 'a as an open quote would swallow
the rest of the file.

RESOLUTION RULES THE LANGUAGE DEMANDS
  * `mod x;` in a file that OWNS its directory (crate roots and mod.rs) looks
    for x.rs / x/mod.rs beside it; in an ordinary name.rs it looks under
    name/x.rs / name/x/mod.rs. Conflating the two manufactures orphans in
    whichever style the repo uses.
  * `#[path = "..."]` overrides resolution outright, and a cfg_attr can carry
    TWO alternative paths for one mod -- emit every candidate; a candidate
    that resolves is an edge, and unresolved ones are only recorded when NONE
    resolved (they are cfg branches, not failures).
  * A workspace is several crates. `use crate::...` resolves inside the
    referencing file's own tree; `use other_member::...` is a cross-crate edge
    into that member's lib tree (crate names use `-`, module paths use `_`).
  * `[lib] name = "..."` renames the lib target -- main.rs importing the lib
    uses THAT name, not the package name.
  * `include!("literal.rs")` is a real edge. `include!(concat!(env!("OUT_DIR"),
    ...))` targets GENERATED code outside the repo and must not cast doubt on
    repo files; only a non-literal include of repo-shaped paths is opaque.
"""
from __future__ import annotations

import json
import os
import re
from pathlib import Path

SKIP_DIRS = {
    ".git", "target", "node_modules", "dist", "build", "vendor",
    ".venv", "venv", ".idea", ".vscode", "logs", "coverage", "out",
    # Agent worktrees are near-complete repo COPIES whose clones import each
    # other -- they mask true findings, they do not merely inflate counts.
    "worktrees", ".claude", ".hive-manager", ".worktrees",
    # Agent session artifacts: prose that names modules, demoting true
    # findings from `certain` to `likely`.
    ".hive", ".swarm", ".agent-sessions",
}

CRATE_ROOT_STEMS = {"main", "lib", "mod"}


# --------------------------------------------------------------------------
# stripping
# --------------------------------------------------------------------------
def _strip(src: str, keep_strings: bool) -> str:
    """Blank comments (and optionally strings), preserving line structure.

    Rust-specific hazards handled here, each of which broke a naive port of
    the TS stripper on real code:
      * block comments NEST -- `/* outer /* inner */ still comment */`
      * raw strings: r"..." and r#"..."# with any number of hashes
      * lifetimes: 'a is NOT the start of a char literal; only '<char>' or
        '\\...' is. Opening a "string" at a lifetime swallows the file.
    """
    out = []
    i, n = 0, len(src)
    depth = 0            # block-comment nesting depth
    line_comment = False
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if line_comment:
            if c == "\n":
                line_comment = False
                out.append(c)
            else:
                out.append(" ")
            i += 1
            continue
        if depth:
            if c == "/" and nxt == "*":
                depth += 1
                out.append("  ")
                i += 2
                continue
            if c == "*" and nxt == "/":
                depth -= 1
                out.append("  ")
                i += 2
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        if c == "/" and nxt == "/":
            line_comment = True
            out.append("  ")
            i += 2
            continue
        if c == "/" and nxt == "*":
            depth = 1
            out.append("  ")
            i += 2
            continue
        if c == "r" and (nxt == '"' or nxt == "#"):
            # raw string candidate: r"..." or r#"..."# (any hash count)
            j = i + 1
            hashes = 0
            while j < n and src[j] == "#":
                hashes += 1
                j += 1
            if j < n and src[j] == '"':
                close = '"' + "#" * hashes
                end = src.find(close, j + 1)
                end = n if end == -1 else end + len(close)
                seg = src[i:end]
                if keep_strings:
                    out.append(seg)
                else:
                    out.append("".join("\n" if ch == "\n" else " " for ch in seg))
                i = end
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            seg = src[i:j]
            if keep_strings:
                out.append(seg)
            else:
                # Blank the CONTENT but keep the quotes and the exact length:
                # scan_file re-reads attribute regions from the strings-kept
                # copy using offsets taken from this one, so the two must stay
                # byte-for-byte aligned.
                inner = "".join("\n" if ch == "\n" else " " for ch in seg[1:-1])
                out.append(seg[:1] + inner + seg[-1:] if len(seg) > 1 else seg)
            i = j
            continue
        if c == "'":
            # char literal ('x', '\n', '\u{1F600}') vs lifetime ('a). A char
            # literal always closes within a few chars; a lifetime never has a
            # closing quote adjacent. Decide by looking for the close.
            if nxt == "\\":
                j = i + 2
                while j < n and src[j] != "'":
                    j += 1
                i = j + 1
                out.append("''")
                continue
            if i + 2 < n and src[i + 2] == "'" and nxt != "'":
                out.append("'" + (" " if not keep_strings else nxt) + "'")
                i += 3
                continue
            # lifetime -- emit as-is, it is code
            out.append(c)
            i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def strip_code(src: str) -> str:
    """Comments AND string contents blanked -- for scanning mod/use items."""
    return _strip(src, keep_strings=False)


def strip_comments_only(src: str) -> str:
    """Comments blanked, strings kept -- #[path]/include! targets live in
    strings, exactly as import specifiers did in the TS phase."""
    return _strip(src, keep_strings=True)


# --------------------------------------------------------------------------
# item scanners, run on stripped source
# --------------------------------------------------------------------------
# attribute blob (possibly several, possibly multiline) + `mod name;`
RX_MOD = re.compile(
    r"((?:#\[[^\]]*\]\s*)*)"
    r"(?:pub(?:\s*\([^)]*\))?\s+)?\bmod\s+([A-Za-z_]\w*)\s*;")
RX_ATTR_PATH = re.compile(r'path\s*=\s*"([^"]+)"')
RX_USE = re.compile(
    r"(?:pub(?:\s*\([^)]*\))?\s+)?\buse\s+([^;]+);")
RX_EXTERN_CRATE = re.compile(r"\bextern\s+crate\s+([A-Za-z_]\w*)")
RX_INCLUDE = re.compile(r'\binclude!\s*\(\s*"([^"]+)"\s*\)')
RX_INCLUDE_NONLIT = re.compile(r"\binclude!\s*\(\s*(?!\s*\")")
RX_AUTOMOD = re.compile(r'\bautomod::dir!\s*\(\s*"([^"]+)"\s*\)')
RX_FN_MAIN = re.compile(r"\bfn\s+main\s*\(")


def scan_file(src: str):
    """(mods, uses, includes, automods, opaque_sites) from one file.

    mods: list of (name, [explicit #[path] targets])  -- paths may be empty
    uses: list of path-segment tuples (already brace-expanded)
    includes: list of literal include! targets
    automods: literal directory arguments to automod::dir!, which mounts EVERY
              .rs file in that directory. The argument is a literal, so this is
              statically resolvable and must become real edges -- the same rule
              the Python phase applies to import_module("a.b").
    opaque: list of (bounded_prefix_or_None, kind) for mounts static analysis
            cannot resolve; None prefix means unbounded
    """
    code = strip_code(src)
    strings_kept = strip_comments_only(src)

    mods = []
    for m in RX_MOD.finditer(code):
        # #[path] values are string literals: re-read the attribute region
        # from the strings-kept copy at the same offsets (stripping preserves
        # length line-by-line closely enough only for the blob re-scan, so
        # instead just re-scan the strings-kept text around the same mod name)
        attr_blob = strings_kept[m.start(1):m.end(1)]
        paths = RX_ATTR_PATH.findall(attr_blob)
        mods.append((m.group(2), paths))

    uses = []
    for m in RX_USE.finditer(code):
        uses.extend(expand_use_tree(m.group(1)))
    for m in RX_EXTERN_CRATE.finditer(code):
        uses.append((m.group(1),))

    includes = RX_INCLUDE.findall(strings_kept)
    automods = RX_AUTOMOD.findall(strings_kept)

    opaque = []
    for m in RX_INCLUDE_NONLIT.finditer(strings_kept):
        tail = strings_kept[m.end():m.end() + 120]
        if "OUT_DIR" in tail:
            continue          # generated code outside the repo: bounded away
        opaque.append((None, "include!(<non-literal>)"))

    return mods, uses, includes, automods, opaque


def expand_use_tree(spec: str):
    """`a::b::{c, d::{e, f}, self}` -> [(a,b,c), (a,b,d,e), (a,b,d,f), (a,b)].

    Handles `as` renames (path before `as` is what resolves) and `*` globs
    (the glob's parent is the referenced module).
    """
    spec = spec.strip()
    results = []

    def walk(prefix: tuple, s: str):
        s = s.strip()
        if not s:
            if prefix:
                results.append(prefix)
            return
        brace = s.find("{")
        if brace == -1:
            # plain path, maybe `as rename`
            path = s.split(" as ")[0].strip()
            segs = [p.strip() for p in path.split("::") if p.strip()]
            segs = [p for p in segs if p != "*"]
            full = prefix + tuple(segs)
            full = _fold_self(full)
            if full:
                results.append(full)
            return
        head = [p.strip() for p in s[:brace].split("::") if p.strip()]
        inner = s[brace + 1:_match_brace(s, brace)]
        for part in _split_top(inner):
            walk(prefix + tuple(head), part)

    def _match_brace(s: str, start: int) -> int:
        depth = 0
        for i in range(start, len(s)):
            if s[i] == "{":
                depth += 1
            elif s[i] == "}":
                depth -= 1
                if depth == 0:
                    return i
        return len(s)

    def _split_top(s: str):
        parts, depth, cur = [], 0, []
        for ch in s:
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            if ch == "," and depth == 0:
                parts.append("".join(cur))
                cur = []
            else:
                cur.append(ch)
        if cur:
            parts.append("".join(cur))
        return parts

    def _fold_self(segs: tuple) -> tuple:
        # `a::b::self` means a::b; interior `self` never occurs
        return segs[:-1] if segs and segs[-1] == "self" else segs

    walk((), spec)
    return results


# --------------------------------------------------------------------------
# discovery
# --------------------------------------------------------------------------
def walk_src(root: Path):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if fn.endswith(".rs"):
                yield Path(dirpath) / fn


def find_crates(root: Path):
    """Every directory holding a Cargo.toml, deepest first so files attach to
    their NEAREST crate (workspace members before the workspace root)."""
    crates = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        if "Cargo.toml" in filenames:
            crates.append(Path(dirpath))
    return sorted(crates, key=lambda p: len(p.parts), reverse=True)


def cargo_meta(crate_dir: Path):
    """{lib_name, pkg_name, explicit_target_paths, path_deps} from Cargo.toml.

    Uses stdlib tomllib when available (3.11+); otherwise a section-aware
    line scan that extracts only the handful of keys needed. Either way a
    malformed manifest degrades to defaults, never to a crash.
    """
    p = crate_dir / "Cargo.toml"
    meta = {"pkg_name": crate_dir.name, "lib_name": None,
            "target_paths": [], "path_deps": {}}
    try:
        raw = p.read_text("utf-8", errors="replace")
    except OSError:
        return meta

    data = None
    try:
        import tomllib
        data = tomllib.loads(raw)
    except Exception:
        pass

    if data is not None:
        pkg = data.get("package") or {}
        if isinstance(pkg.get("name"), str):
            meta["pkg_name"] = pkg["name"]
        lib = data.get("lib") or {}
        meta["lib_name"] = lib.get("name") or meta["pkg_name"]
        if isinstance(lib.get("path"), str):
            meta["target_paths"].append(lib["path"])
        for key in ("bin", "test", "bench", "example"):
            for t in data.get(key) or []:
                if isinstance(t, dict) and isinstance(t.get("path"), str):
                    meta["target_paths"].append(t["path"])
        for depkey in ("dependencies", "dev-dependencies", "build-dependencies"):
            for name, v in (data.get(depkey) or {}).items():
                if isinstance(v, dict) and isinstance(v.get("path"), str):
                    meta["path_deps"][name] = v["path"]
        return meta

    # fallback: section-aware line scan
    section = ""
    for line in raw.splitlines():
        s = line.strip()
        mh = re.match(r"^\[+([^\]]+)\]+", s)
        if mh:
            section = mh.group(1).strip()
            continue
        mk = re.match(r'^name\s*=\s*"([^"]+)"', s)
        if mk and section == "package":
            meta["pkg_name"] = mk.group(1)
        if mk and section == "lib":
            meta["lib_name"] = mk.group(1)
        mp = re.match(r'^path\s*=\s*"([^"]+)"', s)
        if mp and section in ("lib", "bin", "test", "bench", "example"):
            meta["target_paths"].append(mp.group(1))
        md = re.match(r'^([\w-]+)\s*=\s*\{[^}]*path\s*=\s*"([^"]+)"', s)
        if md and section.endswith("dependencies"):
            meta["path_deps"][md.group(1)] = md.group(2)
    if meta["lib_name"] is None:
        meta["lib_name"] = meta["pkg_name"]
    return meta


def crate_roots(crate_dir: Path, meta: dict, index: set):
    """[(root_file, reason)] -- every file cargo treats as a compilation
    root for this crate, via auto-discovery plus explicit manifest targets."""
    roots = []

    def add(p: Path, why: str):
        if str(p) in index:
            roots.append((p, why))

    add(crate_dir / "src" / "lib.rs", "lib-root")
    add(crate_dir / "src" / "main.rs", "bin-root")
    add(crate_dir / "build.rs", "build-script")
    for sub, why in (("bin", "bin-root"), ("examples", "example"),
                     ("tests", "integration-test"), ("benches", "bench")):
        base = crate_dir / ("src/bin" if sub == "bin" else sub)
        if not base.is_dir():
            continue
        try:
            for child in sorted(base.iterdir()):
                if child.is_file() and child.suffix == ".rs":
                    add(child, why)
                elif child.is_dir():
                    add(child / "main.rs", why)
        except OSError:
            pass
    for t in meta["target_paths"]:
        add((crate_dir / t).resolve(), "cargo-manifest-target")
    return roots


# --------------------------------------------------------------------------
# graph build
# --------------------------------------------------------------------------
def _owns_parent(f: Path, is_root: bool) -> bool:
    """Does `mod x;` in this file look in f.parent (roots, mod.rs) or in
    f.parent/<stem> (ordinary name.rs)? The single most consequential rule in
    Rust resolution -- see module docstring."""
    return is_root or f.name == "mod.rs"


def _automod_dir(spec: str, f: Path, crate_dir: Path | None, root: Path):
    """Resolve an automod::dir! argument to a real directory, or None.

    The argument is documented as relative to the crate root, but repos in the
    wild also write it relative to the workspace root or the declaring file.
    Try each; the first that EXISTS wins. Returning None matters as much as
    returning a hit: an unresolvable directory mount is genuinely opaque, and
    the caller must cap tiers rather than quietly drop the mount -- dropping it
    would let a file this macro actually compiles be reported `certain`.
    """
    bases = [b for b in (crate_dir, root, f.parent) if b is not None]
    for b in bases:
        cand = (b / spec).resolve()
        if cand.is_dir():
            return cand
    return None


def _mod_candidates(f: Path, name: str, paths: list, is_root: bool):
    """Filesystem candidates for `mod name;` declared in f."""
    if paths:
        # #[path] overrides everything; cfg_attr may supply several
        return [(f.parent / p).resolve() for p in paths]
    base = f.parent if _owns_parent(f, is_root) else f.parent / f.stem
    return [(base / f"{name}.rs").resolve(),
            (base / name / "mod.rs").resolve()]


def build(root: Path) -> dict:
    root = root.resolve()
    files = sorted(walk_src(root))
    index = {str(f) for f in files}
    crates = find_crates(root)

    nodes = {}
    scans = {}
    for f in files:
        try:
            src = f.read_text("utf-8", errors="replace")
        except OSError:
            continue
        key = str(f)
        nodes[key] = {
            "path": str(f.relative_to(root)),
            "loc": src.count("\n") + 1,
            "entrypoint": [],
            "_src": src,
        }
        scans[key] = scan_file(src)

    # nearest crate for each file (crates sorted deepest-first)
    def crate_of(f: Path):
        for c in crates:
            if str(f).startswith(str(c) + os.sep):
                return c
        return None

    metas = {str(c): cargo_meta(c) for c in crates}
    # crate module-name -> (crate_dir, lib root file), for cross-crate `use`
    lib_by_name = {}
    for c in crates:
        m = metas[str(c)]
        lib_file = c / "src" / "lib.rs"
        for nm in {m["pkg_name"], m["lib_name"]}:
            if nm:
                lib_by_name[nm.replace("-", "_")] = (c, lib_file)

    edges = set()
    unresolved = {}
    opaque_prefixes, opaque_sites = [], []
    automod_blind = []       # (declaring file, unresolvable dir spec)
    root_files = {}          # file key -> [reasons]

    for c in crates:
        for rf, why in crate_roots(c, metas[str(c)], index):
            root_files.setdefault(str(rf), []).append(why)

    # No Cargo.toml anywhere: a bare tree of .rs scripts. Fall back to
    # blessing fn-main files as roots rather than inventing findings out of
    # an absent manifest -- the same rule the Python phase applies.
    no_manifest = not crates
    if no_manifest:
        for key, n in nodes.items():
            if RX_FN_MAIN.search(strip_code(n["_src"])):
                root_files.setdefault(key, []).append("script-no-manifest")

    # ---- mount expansion: BFS from each root, assigning module paths -------
    # modpaths: file key -> {(crate_key, modpath_tuple), ...}
    modpaths = {}
    mounted = set()
    for rkey in root_files:
        rf = Path(rkey)
        c = crate_of(rf)
        ckey = str(c) if c else ""
        queue = [(rf, (), True)]
        seen_local = set()
        while queue:
            f, mpath, is_root = queue.pop()
            fkey = str(f)
            if fkey in seen_local:
                continue
            seen_local.add(fkey)
            mounted.add(fkey)
            modpaths.setdefault(fkey, set()).add((ckey, mpath))
            if fkey not in scans:
                continue
            mods, _uses, includes, automods, _opaque = scans[fkey]
            for name, paths in mods:
                cands = [p for p in _mod_candidates(f, name, paths, is_root)]
                hit = [p for p in cands if str(p) in index]
                for h in hit:
                    edges.add((fkey, str(h)))
                    queue.append((h, mpath + (name,), False))
                if not hit:
                    nrel = nodes[fkey]["path"]
                    unresolved.setdefault(nrel, []).append(f"mod {name};")
            for inc in includes:
                t = (f.parent / inc).resolve()
                if str(t) in index:
                    edges.add((fkey, str(t)))
                    queue.append((t, mpath, is_root))
            # automod::dir!("d") mounts EVERY .rs in d. Literal argument ->
            # statically resolvable -> real edges, not a tier downgrade.
            for spec in automods:
                d = _automod_dir(spec, f, c, root)
                if d is None:
                    automod_blind.append((nodes[fkey]["path"], spec))
                    continue
                for child in sorted(d.glob("*.rs")):
                    if str(child) in index and str(child) != fkey:
                        edges.add((fkey, str(child)))
                        queue.append((child, mpath + (child.stem,), False))

    # ---- mod edges from UNMOUNTED files too --------------------------------
    # If the parent of an orphan subtree is dead, its children are dead with
    # it -- but the edges still exist and dropping them silently would hide
    # the subtree's structure. Recording them makes the children `suspect`
    # ("has importers but unreachable"), which is the honest tier: delete the
    # parent and they resolve together.
    for fkey, n in nodes.items():
        if fkey in mounted:
            continue
        f = Path(fkey)
        mods, _uses, includes, _automods, _opaque = scans.get(
            fkey, ([], [], [], [], []))
        for name, paths in mods:
            is_rootish = f.stem in CRATE_ROOT_STEMS
            hit = [p for p in _mod_candidates(f, name, paths, is_rootish)
                   if str(p) in index]
            for h in hit:
                edges.add((fkey, str(h)))
        for inc in includes:
            t = (f.parent / inc).resolve()
            if str(t) in index:
                edges.add((fkey, str(t)))

    # ---- use / extern crate edges -----------------------------------------
    # Build per-crate module index: (crate_key, modpath) -> file key
    tree = {}
    for fkey, mps in modpaths.items():
        for ck, mp in mps:
            tree.setdefault((ck, mp), fkey)

    def resolve_use(fkey: str, segs: tuple):
        """Resolve a use path to an internal file key, or None if external.
        Returns (target|None, looked_internal)."""
        f = Path(fkey)
        c = crate_of(f)
        ckey = str(c) if c else ""
        own = sorted(mp for ck, mp in modpaths.get(fkey, ()) if ck == ckey)

        if not segs:
            return None, False
        head = segs[0]
        if head in ("crate", "self", "super"):
            if not own:
                return None, True     # unmounted file using crate:: paths
            base = own[0]
            rest = list(segs)
            if head == "crate":
                base = ()
                rest = rest[1:]
            elif head == "self":
                rest = rest[1:]
            else:
                while rest and rest[0] == "super":
                    base = base[:-1] if base else base
                    rest = rest[1:]
            return _longest(ckey, tuple(base) + tuple(rest)), True
        if head == "":
            return None, False
        # bare first segment: a workspace member (or this crate's own lib,
        # e.g. main.rs doing `use my_lib_name::...`) -- else external
        target = lib_by_name.get(head)
        if target is None:
            return None, False
        tc, tlib = target
        hit = _longest(str(tc), tuple(segs[1:]))
        if hit:
            return hit, True
        return (str(tlib) if str(tlib) in index else None), True

    def _longest(ckey: str, segs: tuple):
        for i in range(len(segs), -1, -1):
            hit = tree.get((ckey, segs[:i]))
            if hit:
                return hit
        return None

    for fkey, n in nodes.items():
        _mods, uses, _includes, _automods, opaq = scans.get(
            fkey, ([], [], [], [], []))
        for _pref, _kind in opaq:
            # A non-literal include! can pull in anything, so it is the only
            # case that justifies casting doubt on the whole crate.
            opaque_prefixes.append("")
            opaque_sites.append([f"{n['path']}", ""])
        for segs in uses:
            target, internal = resolve_use(fkey, segs)
            if target and target != fkey:
                edges.add((fkey, target))
            elif internal and target is None:
                unresolved.setdefault(n["path"], []).append("::".join(segs))

    # An automod::dir! whose directory we could not locate is a mount we know
    # exists but cannot follow. Recording it as unbounded is the honest call:
    # dropping it would let a file that macro really does compile be reported
    # `certain`, which is the one error this tool must never make.
    for decl_path, spec in automod_blind:
        opaque_prefixes.append("")
        opaque_sites.append([f"{decl_path} (automod::dir!(\"{spec}\"))", ""])

    # ---- entrypoints and runnability --------------------------------------
    for fkey, reasons in root_files.items():
        if fkey in nodes:
            nodes[fkey]["entrypoint"] = sorted(set(reasons))
    for fkey, n in nodes.items():
        code = strip_code(n["_src"])
        # Runnable by hand (`rustc file.rs` / copied into a bin target) but
        # wired to no cargo target -- the Rust shape of `unwired`.
        n["runnable"] = bool(
            RX_FN_MAIN.search(code) and not n["entrypoint"] and not no_manifest
        )

    # ---- reachability BFS --------------------------------------------------
    fwd, rev = {}, {}
    for a, b in edges:
        fwd.setdefault(a, set()).add(b)
        rev.setdefault(b, set()).add(a)
    roots_ = [k for k, n in nodes.items() if n["entrypoint"]]
    seen, queue = set(roots_), list(roots_)
    while queue:
        cur = queue.pop()
        for nxt in fwd.get(cur, ()):
            if nxt not in seen:
                seen.add(nxt)
                queue.append(nxt)

    out_nodes = {}
    for k, n in nodes.items():
        n["reachable"] = k in seen
        n["mounted"] = k in mounted
        n["importers"] = sorted(nodes[i]["path"] for i in rev.get(k, ())
                                if i in nodes)
        n["imports"] = sorted(nodes[i]["path"] for i in fwd.get(k, ())
                              if i in nodes)
        n.pop("_src", None)
        out_nodes[n["path"]] = n

    return {
        "root": str(root),
        "language": "rust",
        "counts": {
            "modules": len(nodes),
            "edges": len(edges),
            "entrypoints": len(roots_),
            "reachable": len(seen),
            "unreached": len(nodes) - len(seen),
            "unresolved_imports": sum(len(v) for v in unresolved.values()),
            "parse_errors": 0,
        },
        "crates": sorted(str(Path(c).relative_to(root)) if c != root else "."
                         for c in (str(x) for x in crates)),
        "nodes": out_nodes,
        "edges": sorted([nodes[a]["path"], nodes[b]["path"]]
                        for a, b in edges if a in nodes and b in nodes),
        "unresolved": unresolved,
        "parse_errors": {},
        "opaque_prefixes": sorted({p for p in opaque_prefixes if p}),
        "opaque_unbounded": any(p == "" for p in opaque_prefixes),
        "opaque_sites": sorted(opaque_sites),
    }


# --------------------------------------------------------------------------
# dead-code tiering (mirrors the TS phase; only leaf-name derivation and the
# corpus extensions differ)
# --------------------------------------------------------------------------
TEXT_SCAN_EXT = (".rs", ".toml", ".md", ".yml", ".yaml", ".json", ".sh",
                 ".txt", "dockerfile", "makefile")


def leaf_name(rel: str) -> str:
    """The name other code would reference this file by. mod.rs, and the
    main.rs of a bin dir, are referenced by their DIRECTORY's name."""
    parts = rel.replace("\\", "/").split("/")
    stem = parts[-1][:-3] if parts[-1].endswith(".rs") else parts[-1]
    if stem in CRATE_ROOT_STEMS and len(parts) > 1:
        return parts[-2]
    return stem


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
        stem = leaf_name(rel)
        rx = re.compile(rf"\b{re.escape(stem)}\b")
        hits = sum(1 for other, text in corpus
                   if other != rel and rx.search(text))

        abs_mod = str((root / rel).resolve())
        shadow = None
        if opaque_unbounded:
            shadow = "repo has an include!/mount whose target could not be bounded"
        else:
            for p in opaque_prefixes:
                if abs_mod.startswith(p):
                    shadow = f"reachable via directory mount under `{p}*`"
                    break

        if shadow:
            tier, why = "suspect", shadow
        elif n.get("runnable"):
            tier, why = "unwired", ("has fn main(), but is no cargo target "
                                    "and sits in no auto-discovered location")
        elif n["importers"]:
            tier, why = "suspect", ("has importers but is not reachable from "
                                    "any crate root")
        elif hits == 0:
            tier, why = "certain", ("never mounted by any `mod`, no crate "
                                    "root, zero textual references repo-wide")
        else:
            tier, why = "likely", (f"never mounted, but name appears in "
                                   f"{hits} file(s)")

        results.append({"module": rel, "path": rel, "loc": n["loc"],
                        "tier": tier, "why": why, "text_refs": hits,
                        "tracked": (None if tracked is None else rel in tracked)})
    return results
