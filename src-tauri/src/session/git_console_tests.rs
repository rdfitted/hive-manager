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
    let cfg = Regex::new(r"#\s*\[\s*cfg\s*\([^\]]+\)\s*\]").unwrap();
    let ranges: Vec<_> = cfg.find_iter(&code).filter_map(|attr| {
        let compact: String = attr.as_str().chars().filter(|c| !c.is_whitespace()).collect();
        // Only test-required items are excluded. any(unix, test) is production.
        let test_only = compact == "#[cfg(test)]"
            || compact.strip_prefix("#[cfg(all(").is_some_and(|body| {
                body.trim_end_matches(")]" ).trim_end_matches(')').split(',').any(|term| term == "test")
            });
        if !test_only { return None; }
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
    let sites = Regex::new(r"\bCommand\s*::\s*new\s*\(").unwrap();
    let functions = Regex::new(r"\bfn\s+(\w+)\s*\(").unwrap();
    let mut count = 0;
    let mut failures = Vec::new();
    for site in sites.find_iter(&code) {
        count += 1;
        let function = functions.captures_iter(&code[..site.start()]).last().unwrap();
        let name = &function[1];
        let open = code[function.get(0).unwrap().end()..].find('{').unwrap()
            + function.get(0).unwrap().end();
        let end = closing_brace(&code, open);
        let before = &code[open..site.start()];
        let binding = Regex::new(r"let\s+(?:mut\s+)?(\w+)\s*=\s*(?:(?:std|tokio)::process::)?$").unwrap();
        let variable = binding.captures(before).map(|c| c[1].to_owned());
        let hidden = variable.as_ref().is_some_and(|variable| {
            let tail = &code[site.end()..end];
            let execution = Regex::new(&format!(
                r"\b{}\s*(?:\.[^;]*?)?\.\s*(?:output|spawn|status)\s*\(", regex::escape(variable)
            )).unwrap();
            let first_execution = execution.find(tail).map_or(tail.len(), |call| call.start());
            let helper = Regex::new(&format!(
                r"hide_(?:std|tokio)_console_window\s*\(\s*&mut\s+{}\s*\)",
                regex::escape(variable)
            )).unwrap();
            helper.find(tail).is_some_and(|call| call.start() < first_execution)
        });
        let tail = &code[site.end()..end];
        let execution = Regex::new(r"\.(?:output|spawn|status)\s*\(").unwrap();
        let inline_flag = Regex::new(r"\.creation_flags\s*\(\s*(CREATE_NO_WINDOW|0x0?800_?0000)\b").unwrap();
        let inline_hidden = inline_flag.captures_iter(tail).any(|flag| {
            let position = flag.get(0).unwrap().start();
            // Chained flags belong to the constructor, or to this binding.
            let receiver = Regex::new(&format!(r"\b{}\s*$", regex::escape(variable.as_deref().unwrap_or("")))).unwrap();
            let same_builder = !tail[..position].contains(';') || variable.is_some() && receiver.is_match(&tail[..position]);
            let value_verified = &flag[1] != "CREATE_NO_WINDOW" || Regex::new(
                r"const\s+CREATE_NO_WINDOW\s*:\s*u32\s*=\s*0x0?800_?0000\s*;"
            ).unwrap().is_match(&code);
            same_builder && value_verified && !execution.is_match(&tail[..position])
        });
        // Reasoned platform allowlist. Match the exact cfg as well as the name,
        // so the Windows overload cannot inherit a non-Windows exemption.
        let platform_exceptions = [
            ("cli/env_path.rs", "probe_login_shell", "#[cfg(any(target_os = \"macos\", all(test, unix)))]",
             "macOS login shell; Unix-only fixture on other platforms"),
            ("cli/health.rs", "build_probe_command", "#[cfg(not(windows))]",
             "non-Windows overload; Windows overloads must hide their commands"),
        ];
        let exempt = platform_exceptions.iter().any(|(file, function_name, cfg, _reason)| {
            path == *file && name == *function_name
                && source[..function.get(0).unwrap().start()].trim_end().ends_with(cfg)
        });
        if !hidden && !inline_hidden && !exempt {
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
        if path.is_dir() {
            scan(root, &path, total, failures);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if test_only_file(root, &path, &relative) { continue; }
            let (count, found) = violations(&relative, &fs::read_to_string(&path).unwrap());
            *total += count;
            failures.extend(found);
        }
    }
}

fn test_only_file(root: &Path, path: &Path, relative: &str) -> bool {
    let module = path.file_stem().unwrap().to_str().unwrap();
    let parent = match relative {
        "session/lane_a_tests.rs" | "session/lane_e_tests.rs" => "session/controller.rs".to_owned(),
        "session/git_console_tests.rs" => "session/retirement.rs".to_owned(),
        "commands/lane_e_resume_tests.rs" => "commands/session_commands.rs".to_owned(),
        "instance_fixture_tests.rs" => "lib.rs".to_owned(),
        _ if module == "tests" || module == "resume_tests" || module.starts_with("tests_")
            || module.ends_with("_tests") => {
                format!("{}/mod.rs", relative.rsplit_once('/').map_or("", |(dir, _)| dir))
            }
        _ => return false,
    };
    let Ok(source) = fs::read_to_string(root.join(&parent)) else { return false; };
    let production = production_code(&source);
    let declaration = Regex::new(&format!(r"\bmod\s+{}\s*[;{{]", regex::escape(module))).unwrap();
    let included = Regex::new(&format!(r#"\binclude!\s*\(\s*"{}\.rs"\s*\)"#, regex::escape(module))).unwrap();
    let sites: Vec<_> = declaration.find_iter(&source).chain(included.find_iter(&source)).collect();
    // The exemption is valid only while every declaration/include is masked by
    // an enclosing test-required cfg. A filename alone never exempts a spawn.
    !sites.is_empty() && sites.iter().all(|site| production[site.range()].trim().is_empty())
}

#[test]
fn production_git_commands_hide_console_windows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut count = 0;
    let mut failures = Vec::new();
    scan(&root, &root, &mut count, &mut failures);
    assert!(count > 0, "production command inventory unexpectedly empty");
    assert!(failures.is_empty(), "unhidden production commands:\n{}", failures.join("\n"));
    println!("{count} production command sites checked (two cfg-verified platform exceptions)");
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
    for constructor in ["std::process::Command", "tokio::process::Command"] {
        let unhidden = format!("fn probe() {{ let mut cmd = {constructor}::new(program); cmd.spawn(); }}");
        assert_eq!(violations("probe.rs", &unhidden).1.len(), 1);
        let late = unhidden.replace("cmd.spawn();", "cmd.spawn(); crate::process::hide_std_console_window(&mut cmd);");
        assert_eq!(violations("probe.rs", &late).1.len(), 1);
        assert_eq!(violations("probe.rs", &format!("#[cfg(all(test, windows))] mod tests {{ {unhidden} }}")).0, 0);
        assert_eq!(violations("probe.rs", &format!("#[cfg(any(unix, test))] {unhidden}")).1.len(), 1);
    }
    let inline = "fn probe() { let child = std::process::Command::new(program).creation_flags(0x0800_0000).spawn(); }";
    assert!(violations("probe.rs", inline).1.is_empty());
    assert_eq!(violations("probe.rs", &inline.replace(".creation_flags(0x0800_0000)", "")).1.len(), 1);
    let late_flags = "fn probe() { let mut cmd = tokio::process::Command::new(program); cmd.spawn(); cmd.creation_flags(0x08000000); }";
    assert_eq!(violations("probe.rs", late_flags).1.len(), 1);
    let late_chained = "fn probe() { let mut cmd = std::process::Command::new(program); cmd.args([]).output(); crate::process::hide_std_console_window(&mut cmd); }";
    assert_eq!(violations("probe.rs", late_chained).1.len(), 1);
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
