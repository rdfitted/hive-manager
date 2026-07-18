//! Compile-time parity guard for the Tauri v2 command ACL (PR #154 invariant).
//!
//! Tauri v2 denies any `invoke` whose command is not explicitly allowed by a
//! capability. `capabilities/default.json` grants the main window the
//! `main-window-commands` permission, whose `commands.allow` list is the single
//! source of truth for what the main window may call. If a command is added to
//! `tauri::generate_handler![...]` but not to that manifest, the build is clean,
//! clippy is clean, CI is green — and the feature dies with an ACL denial the
//! first time a user clicks it.
//!
//! This test reads both lists out of the real source at compile time via
//! `include_str!` and asserts they describe the same SET of commands.
//!
//! Parsing notes (these are load-bearing, do not "simplify" them):
//!  * The handler block is located with real bracket-depth matching. A non-greedy
//!    regex such as `generate_handler!\s*\[(.*?)\]` truncates at the first `]` it
//!    finds and silently yields a short list — a parser that reads the wrong
//!    block is worse than no test at all.
//!  * Comparison is on the bare final path segment, so `preview::open_preview_window`
//!    and the manifest's `"open_preview_window"` match.
//!  * Comparison is SET-based. The two lists are set-equal but NOT order-equal.

use std::collections::{BTreeSet, HashMap};

/// The real `lib.rs` source, captured at compile time.
const LIB_RS: &str = include_str!("lib.rs");

/// The real capability permission manifest, captured at compile time.
const MANIFEST_TOML: &str = include_str!("../permissions/main-window-commands.toml");

/// Non-zero floor so a parser that silently matches nothing can never pass green.
/// The real count was 52 when this test was written; this only needs to be a
/// number a broken parser could not plausibly reach.
const MIN_EXPECTED_COMMANDS: usize = 50;

/// Comment style for the scanner below.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentStyle {
    /// Rust: `//` to end of line, plus `/* ... */`.
    Rust,
    /// TOML: `#` to end of line.
    Toml,
}

/// Returns the byte range of the *contents* of the bracketed block whose opening
/// `[` is at `open_idx`, using true bracket-depth matching.
///
/// Brackets inside comments and inside double-quoted strings are ignored, so a
/// `// [note]` or `"a]b"` inside the block cannot terminate it early.
fn matching_bracket_body(src: &str, open_idx: usize, style: CommentStyle) -> Option<(usize, usize)> {
    let bytes = src.as_bytes();
    if bytes.get(open_idx) != Some(&b'[') {
        return None;
    }

    let mut depth: usize = 0;
    let mut i = open_idx;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string = false;

    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();

        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && next == Some(b'/') {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        match (style, c, next) {
            (CommentStyle::Rust, b'/', Some(b'/')) => {
                in_line_comment = true;
                i += 2;
                continue;
            }
            (CommentStyle::Rust, b'/', Some(b'*')) => {
                in_block_comment = true;
                i += 2;
                continue;
            }
            (CommentStyle::Toml, b'#', _) => {
                in_line_comment = true;
                i += 1;
                continue;
            }
            (_, b'"', _) => {
                in_string = true;
                i += 1;
                continue;
            }
            (_, b'[', _) => depth += 1,
            (_, b']', _) => {
                depth -= 1;
                if depth == 0 {
                    return Some((open_idx + 1, i));
                }
            }
            _ => {}
        }
        i += 1;
    }

    None
}

/// Strips comments (`//` and `/* */` for Rust, `#` for TOML) from a block body,
/// preserving newlines so adjacent entries can never be glued together.
///
/// Uses the same state machine as the bracket matcher, so a comma or bracket
/// inside a comment cannot be mistaken for a separator.
fn strip_comments(body: &str, style: CommentStyle) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0usize;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string = false;

    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();

        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && next == Some(b'/') {
                in_block_comment = false;
                i += 2;
                continue;
            }
            if c == b'\n' {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if in_string {
            if c == b'\\' {
                // Copy the backslash, then the escaped character whole.
                out.push('\\');
                i = copy_one_char(body, i + 1, &mut out);
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
        } else {
            match (style, c, next) {
                (CommentStyle::Rust, b'/', Some(b'/')) => {
                    in_line_comment = true;
                    i += 2;
                    continue;
                }
                (CommentStyle::Rust, b'/', Some(b'*')) => {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
                (CommentStyle::Toml, b'#', _) => {
                    in_line_comment = true;
                    i += 1;
                    continue;
                }
                (_, b'"', _) => in_string = true,
                _ => {}
            }
        }

        i = copy_one_char(body, i, &mut out);
    }

    out
}

/// Copies the single UTF-8 character starting at byte index `i` into `out` and
/// returns the index just past it. Keeps multi-byte sequences intact.
fn copy_one_char(src: &str, i: usize, out: &mut String) -> usize {
    let bytes = src.as_bytes();
    if i >= bytes.len() {
        return i;
    }
    let mut end = i + 1;
    while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
        end += 1;
    }
    out.push_str(&src[i..end]);
    end
}

/// Splits a comma-separated block into trimmed, non-empty entries.
/// Trailing commas and arbitrary whitespace/newlines are absorbed by the trim.
fn split_entries(body: &str) -> Vec<String> {
    body.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// `preview::open_preview_window` -> `open_preview_window`
fn bare_command_name(path: &str) -> String {
    path.rsplit("::")
        .next()
        .unwrap_or(path)
        .trim()
        .to_string()
}

/// Commands registered in `tauri::generate_handler![...]` in `lib.rs`.
fn handler_commands() -> Vec<String> {
    let occurrences = LIB_RS.match_indices("generate_handler!").count();
    assert_eq!(
        occurrences, 1,
        "expected exactly one `generate_handler!` invocation in lib.rs, found {occurrences}. \
         This test must parse THE command registry; disambiguate before proceeding."
    );

    let macro_idx = LIB_RS
        .find("generate_handler!")
        .expect("checked by the assert above");
    let open_idx = LIB_RS[macro_idx..]
        .find('[')
        .map(|offset| macro_idx + offset)
        .expect("`generate_handler!` must be followed by a `[` command list");

    let (start, end) = matching_bracket_body(LIB_RS, open_idx, CommentStyle::Rust)
        .expect("unbalanced brackets in the `generate_handler!` block in lib.rs");

    let cleaned = strip_comments(&LIB_RS[start..end], CommentStyle::Rust);
    split_entries(&cleaned)
        .iter()
        .map(|entry| bare_command_name(entry))
        .collect()
}

/// Commands allowed by `permissions/main-window-commands.toml` (`commands.allow`).
fn manifest_commands() -> Vec<String> {
    let key_idx = MANIFEST_TOML
        .find("commands.allow")
        .expect("`commands.allow` key missing from main-window-commands.toml");
    let open_idx = MANIFEST_TOML[key_idx..]
        .find('[')
        .map(|offset| key_idx + offset)
        .expect("`commands.allow` must be assigned a `[` array");

    let (start, end) = matching_bracket_body(MANIFEST_TOML, open_idx, CommentStyle::Toml)
        .expect("unbalanced brackets in `commands.allow` in main-window-commands.toml");

    let cleaned = strip_comments(&MANIFEST_TOML[start..end], CommentStyle::Toml);
    split_entries(&cleaned)
        .iter()
        .map(|entry| {
            let unquoted = entry.trim_matches(|c| c == '"' || c == '\'');
            assert!(
                !unquoted.is_empty(),
                "empty entry in `commands.allow`: {entry:?}"
            );
            assert!(
                entry.starts_with('"') || entry.starts_with('\''),
                "unquoted entry in `commands.allow`: {entry:?}"
            );
            bare_command_name(unquoted)
        })
        .collect()
}

/// Names appearing more than once, with their counts, sorted for stable output.
fn duplicates(items: &[String]) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in items {
        *counts.entry(item.as_str()).or_insert(0) += 1;
    }
    let mut dupes: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, count)| (name.to_string(), count))
        .collect();
    dupes.sort();
    dupes
}

fn format_list(names: &[&String]) -> String {
    if names.is_empty() {
        return "    (none)".to_string();
    }
    names
        .iter()
        .map(|name| format!("    - {name}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The load-bearing invariant: every command the main window can invoke must be
/// allowed by the capability manifest, and vice versa.
#[test]
fn generate_handler_and_acl_manifest_are_set_equal() {
    let handler = handler_commands();
    let manifest = manifest_commands();

    // Floor: a parser that silently matched nothing must never pass green.
    assert!(
        handler.len() >= MIN_EXPECTED_COMMANDS,
        "parsed only {} command(s) from `generate_handler!` in lib.rs, expected at least {}. \
         The parser is broken (or the command registry shrank drastically).",
        handler.len(),
        MIN_EXPECTED_COMMANDS
    );
    assert!(
        manifest.len() >= MIN_EXPECTED_COMMANDS,
        "parsed only {} command(s) from `commands.allow` in main-window-commands.toml, \
         expected at least {}. The parser is broken (or the manifest was gutted).",
        manifest.len(),
        MIN_EXPECTED_COMMANDS
    );

    // Duplicates would let a count-only check hide a missing entry.
    let handler_dupes = duplicates(&handler);
    assert!(
        handler_dupes.is_empty(),
        "duplicate command(s) in `generate_handler!` in lib.rs: {handler_dupes:?}"
    );
    let manifest_dupes = duplicates(&manifest);
    assert!(
        manifest_dupes.is_empty(),
        "duplicate command(s) in `commands.allow` in main-window-commands.toml: {manifest_dupes:?}"
    );

    let handler_set: BTreeSet<String> = handler.iter().cloned().collect();
    let manifest_set: BTreeSet<String> = manifest.iter().cloned().collect();

    if handler_set != manifest_set {
        let missing_from_manifest: Vec<&String> =
            handler_set.difference(&manifest_set).collect();
        let missing_from_handler: Vec<&String> =
            manifest_set.difference(&handler_set).collect();

        panic!(
            "\nTauri command ACL parity violation (PR #154 invariant).\n\
             \n\
             `tauri::generate_handler!` in src-tauri/src/lib.rs registered {handler_len} command(s).\n\
             `commands.allow` in src-tauri/permissions/main-window-commands.toml allowed {manifest_len} command(s).\n\
             \n\
             Registered in generate_handler! but MISSING from the ACL manifest\n\
             (these will fail at runtime with an ACL denial the first time a user invokes them);\n\
             fix by adding each to commands.allow in main-window-commands.toml:\n\
             {missing_from_manifest}\n\
             \n\
             Allowed by the ACL manifest but NOT registered in generate_handler!\n\
             (stale grant, or the command was renamed/removed);\n\
             fix by removing each from commands.allow, or re-registering it in lib.rs:\n\
             {missing_from_handler}\n",
            handler_len = handler.len(),
            manifest_len = manifest.len(),
            missing_from_manifest = format_list(&missing_from_manifest),
            missing_from_handler = format_list(&missing_from_handler),
        );
    }
}

/// Guards the parser itself: proves bracket-depth matching consumes nested
/// brackets and comment noise instead of truncating at the first `]`.
///
/// A non-greedy regex over this fixture stops at `[0]` and reports 1 command;
/// the correct answer is 4.
#[test]
fn bracket_depth_matching_does_not_truncate() {
    let fixture = r#"
        .invoke_handler(tauri::generate_handler![
            // PTY commands [see docs]
            create_pty,
            some_module::nested_arrays[0],
            /* block comment with ] and , inside */
            preview::open_preview_window,
            last_command,
        ])
    "#;

    let macro_idx = fixture.find("generate_handler!").unwrap();
    let open_idx = macro_idx + fixture[macro_idx..].find('[').unwrap();
    let (start, end) =
        matching_bracket_body(fixture, open_idx, CommentStyle::Rust).expect("balanced brackets");

    let cleaned = strip_comments(&fixture[start..end], CommentStyle::Rust);
    let names: Vec<String> = split_entries(&cleaned)
        .iter()
        .map(|entry| bare_command_name(entry))
        .collect();

    assert_eq!(
        names,
        vec![
            "create_pty",
            "nested_arrays[0]",
            "open_preview_window",
            "last_command",
        ],
        "bracket-depth matching truncated or mis-split the handler block"
    );
}
