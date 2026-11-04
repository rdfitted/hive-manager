use std::{fs, path::Path};

use regex::Regex;

// Preserve byte offsets while masking comments and literals. Braces in JSON,
// prompts, raw strings and character literals must not change item boundaries.
fn code_mask(source: &str) -> String {
    let raw_strings = (0..=16).rev().map(|count| {
        let hashes = "#".repeat(count);
        format!("r{hashes}\".*?\"{hashes}")
    }).collect::<Vec<_>>().join("|");
    let literals = Regex::new(&format!(
        r##"(?s)//[^\n]*|/\*.*?\*/|{raw_strings}|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])'"##
    )).unwrap();
    let mut bytes = source.as_bytes().to_vec();
    for found in literals.find_iter(source) {
        for byte in &mut bytes[found.range()] {
            if *byte != b'\n' { *byte = b' '; }
        }
    }
    String::from_utf8(bytes).unwrap()
}

fn closing_brace(code: &str, open: usize) -> usize {
    let mut depth = 0;
    for (offset, byte) in code.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 { return offset; }
            }
            _ => {}
        }
    }
    panic!("unbalanced Rust item");
}

fn production_code(source: &str) -> String {
    let code = code_mask(source);
    let cfg = Regex::new(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]").unwrap();
    let ranges: Vec<_> = cfg.find_iter(&code).filter_map(|attr| {
        let rest = &code[attr.end()..];
        let end = rest.find(['{', ';'])? + attr.end();
        Some(attr.start()..if code.as_bytes()[end] == b'{' {
            closing_brace(&code, end) + 1
        } else { end + 1 })
    }).collect();
    let mut bytes = code.into_bytes();
    for range in ranges { bytes[range].fill(b' '); }
    String::from_utf8(bytes).unwrap()
}

fn violations(path: &str, source: &str) -> (usize, Vec<String>) {
    let code = production_code(source);
    let sites = Regex::new(r#"Command\s*::\s*new\s*\(\s*"git"\s*\)"#).unwrap();
    let functions = Regex::new(r"\bfn\s+(\w+)\s*\(").unwrap();
    let mut count = 0;
    let mut failures = Vec::new();
    for site in sites.find_iter(source) {
        if !code[site.start()..].starts_with("Command") { continue; }
        count += 1;
        let function = functions.captures_iter(&code[..site.start()]).last().unwrap();
        let name = &function[1];
        let open = code[function.get(0).unwrap().end()..].find('{').unwrap()
            + function.get(0).unwrap().end();
        let end = closing_brace(&code, open);
        let before = &code[open..site.start()];
        let binding = Regex::new(r"let\s+mut\s+(\w+)\s*=\s*(?:(?:std|tokio)::process::)?$").unwrap();
        let variable = binding.captures(before).map(|c| c[1].to_owned());
        let hidden = variable.as_ref().is_some_and(|variable| {
            let tail = &code[site.end()..end];
            let helper = Regex::new(&format!(
                r"hide_(?:std|tokio)_console_window\s*\(\s*&mut\s+{}\s*\)",
                regex::escape(variable)
            )).unwrap();
            helper.find(tail).is_some_and(|call| {
                // A hide call after execution cannot protect the spawn.
                let execution = Regex::new(r"\.(?:output|spawn|status)\s*\(").unwrap();
                !execution.is_match(&tail[..call.start()])
            })
        });
        // Existing explicit-action sites already set CREATE_NO_WINDOW inline.
        // Follow up by migrating only these functions to the shared helper.
        let inline_exception = matches!((path, name),
            ("session/controller.rs", "worktree_has_local_changes") |
            ("session/slop_gate.rs", "merge_base"));
        let inline_hidden = inline_exception && variable.is_some_and(|variable| {
            let body = &source[site.end()..end];
            body.contains(&format!("{variable}.creation_flags(CREATE_NO_WINDOW)"))
                && body.contains("const CREATE_NO_WINDOW: u32 = 0x08000000")
                || body.contains(&format!("{variable}.creation_flags(0x08000000)"))
        });
        if !hidden && !inline_hidden {
            let line = source[..site.start()].bytes().filter(|b| *b == b'\n').count() + 1;
            failures.push(format!("{path}:{line} ({name})"));
        }
    }
    (count, failures)
}

fn scan(root: &Path, directory: &Path, total: &mut usize, failures: &mut Vec<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if relative == "process" { continue; }
        if path.is_dir() {
            scan(root, &path, total, failures);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            // Out-of-line test modules declared with #[cfg(test)] in http/mod.rs.
            if relative.starts_with("http/tests") {
                let module = path.file_stem().unwrap().to_str().unwrap();
                let declaration = Regex::new(&format!(
                    r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+{module};"
                )).unwrap();
                let parent = fs::read_to_string(root.join("http/mod.rs")).unwrap().replace("\r\n", "\n");
                assert!(declaration.is_match(&parent), "test-only exemption lost: {relative}");
                continue;
            }
            let (count, found) = violations(&relative, &fs::read_to_string(&path).unwrap());
            *total += count;
            failures.extend(found);
        }
    }
}

#[test]
fn production_git_commands_hide_console_windows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut count = 0;
    let mut failures = Vec::new();
    scan(&root, &root, &mut count, &mut failures);
    assert!(count > 0, "production git inventory unexpectedly empty");
    assert!(failures.is_empty(), "unhidden production git commands:\n{}", failures.join("\n"));
    println!("{count} production git sites checked (two verified inline exceptions)");
}

#[test]
fn guard_rejects_unhidden_and_late_hidden_commands() {
    let unhidden = r#"fn probe() { let mut cmd = Command::new("git"); cmd.output(); }"#;
    assert_eq!(violations("probe.rs", unhidden).1.len(), 1);
    let hidden = unhidden.replace("cmd.output()", "crate::process::hide_std_console_window(&mut cmd); cmd.output()");
    assert!(violations("probe.rs", &hidden).1.is_empty());
    assert_eq!(violations("probe.rs", &hidden.replace("&mut cmd", "&mut other")).1.len(), 1);
    let late = unhidden.replace("cmd.output();", "cmd.output(); crate::process::hide_std_console_window(&mut cmd);");
    assert_eq!(violations("probe.rs", &late).1.len(), 1);
    assert_eq!(violations("probe.rs", &format!("#[cfg(test)] mod tests {{ {unhidden} }}")).0, 0);
    assert_eq!(violations("probe.rs", &format!("// {unhidden}\n/* {unhidden} */")).0, 0);
}

#[test]
fn owned_work_command_preserves_literal_arguments() {
    let command = super::owned_work_command(Path::new("fixture"), &["a b.txt", ":(glob)*"]);
    assert_eq!(command.get_program(), "git");
    assert_eq!(command.get_args().collect::<Vec<_>>(), [
        "--literal-pathspecs", "-C", "fixture", "status", "--porcelain",
        "--untracked-files=all", "--", "a b.txt", ":(glob)*",
    ]);
}

#[cfg(windows)]
#[test]
fn owned_work_command_sets_create_no_window() {
    // std::process::Command has no creation_flags getter. Verify the builder's
    // data flow into the real helper and the helper's Windows flag assignment.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let source = fs::read_to_string(root.join("session/retirement.rs")).unwrap();
    let builder = source.split("fn owned_work_command(").nth(1).unwrap()
        .split("\n}").next().unwrap();
    assert!(builder.contains("crate::process::hide_std_console_window(&mut command);"));
    assert!(builder.trim_end().ends_with("command"));
    let helper = fs::read_to_string(root.join("process/mod.rs")).unwrap();
    assert!(helper.contains("#[cfg(windows)]\nconst CREATE_NO_WINDOW: u32 = 0x08000000;")
        || helper.contains("#[cfg(windows)]\r\nconst CREATE_NO_WINDOW: u32 = 0x08000000;"));
    let standard_helper = helper.split("fn hide_std_console_window(").nth(1).unwrap()
        .split("/// Prevent a Tokio").next().unwrap();
    assert!(standard_helper.contains("#[cfg(windows)]"));
    assert!(standard_helper.contains("command.creation_flags(CREATE_NO_WINDOW);"));
}
