//! Tests for `pty::trust_prompt`, against the real captures in `trust_prompt_fixtures/`.
use super::*;

pub(crate) const AGY_PROMPT: &[u8] = include_bytes!("trust_prompt_fixtures/agy-1.5.18-prompt.bin");
pub(crate) const AGY_AFTER: &[u8] = include_bytes!("trust_prompt_fixtures/agy-1.5.18-after-accept.bin");
pub(crate) const CODEX_PROMPT: &[u8] = include_bytes!("trust_prompt_fixtures/codex-0.171.0-prompt.bin");
pub(crate) const CODEX_AFTER: &[u8] = include_bytes!("trust_prompt_fixtures/codex-0.171.0-after-accept.bin");
pub(crate) const CLAUDE_PROMPT: &[u8] = include_bytes!("trust_prompt_fixtures/claude-2.1.332-prompt.bin");

// The scratch worktrees the fixtures were captured in, at 120x40.
pub(crate) const AGY_CWD: &str = r"D:\hm466-scratch\repo-agy-acc2";
pub(crate) const CODEX_CWD: &str = r"D:\hm466-scratch\repo-codex2-acc";
pub(crate) const CODEX_ROOT: &str = r"d:\hm466-scratch\repo-codex2";
pub(crate) const CLAUDE_CWD: &str = r"D:\hm466-scratch\repo-claude-wt1";

/// A captured prompt re-drawn as one clean frame from its rendered rows, with each swap
/// applied to the text. Codex diff-renders its rows (it skips cells that already hold the
/// right character), so its bytes cannot be patched in place.
pub(crate) fn reframe(capture: &[u8], swaps: &[(&str, &str)]) -> Vec<u8> {
    let mut frame = b"\x1b[2J".to_vec();
    for (row, line) in screen(capture).iter().enumerate() {
        let mut line = line.clone();
        for (from, to) in swaps {
            line = line.replace(from, to);
        }
        frame.extend(format!("\x1b[{};1H{line}", row + 1).into_bytes());
    }
    frame
}

pub(crate) fn screen(bytes: &[u8]) -> Vec<String> {
    let mut screen = Screen::new(120, 40);
    screen.feed(bytes);
    screen.lines()
}

/// An output ring view holding `bytes` from absolute offset 0.
fn ring(bytes: &[u8]) -> OutputSnapshot {
    OutputSnapshot { offset_start: 0, offset_end: bytes.len() as u64, data: bytes.to_vec() }
}

/// The view after `bytes` more output at `offset`.
fn more(offset: u64, bytes: &[u8]) -> OutputSnapshot {
    OutputSnapshot { offset_start: offset, offset_end: offset + bytes.len() as u64, data: bytes.to_vec() }
}

fn concat(a: &[u8], b: &[u8]) -> Vec<u8> {
    [a, b].concat()
}

fn guard(cli: TrustCli, t0: Instant) -> TrustPromptGuard {
    TrustPromptGuard::new(cli, 120, 40, t0)
}

#[test]
fn captured_prompts_are_read_with_their_paths_and_highlight() {
    assert_eq!(detect(TrustCli::Antigravity, &screen(AGY_PROMPT)), Some(TrustPrompt {
        cli: TrustCli::Antigravity, folder: AGY_CWD.into(), repo_root: None, trust_highlighted: true,
    }));
    assert_eq!(detect(TrustCli::Codex, &screen(CODEX_PROMPT)), Some(TrustPrompt {
        cli: TrustCli::Codex, folder: CODEX_CWD.into(), repo_root: Some(CODEX_ROOT.into()), trust_highlighted: true,
    }));
    assert_eq!(detect(TrustCli::Claude, &screen(CLAUDE_PROMPT)), Some(TrustPrompt {
        cli: TrustCli::Claude, folder: CLAUDE_CWD.into(), repo_root: None, trust_highlighted: false,
    }));
}

#[test]
fn the_screen_after_each_answer_shows_no_prompt_and_the_cli_working() {
    let agy = screen(&concat(AGY_PROMPT, AGY_AFTER));
    assert_eq!(detect(TrustCli::Antigravity, &agy), None);
    assert!(agy.iter().any(|line| line.trim() == "READY"), "{agy:#?}");
    let codex = screen(&concat(CODEX_PROMPT, CODEX_AFTER));
    assert_eq!(detect(TrustCli::Codex, &codex), None);
    assert!(codex.iter().any(|line| line.trim() == "• READY"), "{codex:#?}");
}

#[test]
fn prompts_are_specific_to_their_cli_and_loose_text_is_not_a_dialog() {
    assert_eq!(detect(TrustCli::Codex, &screen(AGY_PROMPT)), None);
    assert_eq!(detect(TrustCli::Antigravity, &screen(CODEX_PROMPT)), None);
    // The dialog's words without its order and footer: an agent printing a file.
    let printed = b"Do you trust the contents of this project?\r\nAccessing workspace:\r\nD:\\w\r\n> Yes, I trust this folder\r\n";
    assert_eq!(detect(TrustCli::Antigravity, &screen(printed)), None);
    // The same dialog lines spread over more than one dialog's height.
    let spread = [b"Accessing workspace:\r\nD:\\w\r\n".as_slice(), &b"\r\n".repeat(DIALOG_SPAN),
        b"Do you trust the contents of this project?\r\n> Yes, I trust this folder\r\n  No, exit\r\n  Navigate - enter Confirm\r\n"].concat();
    assert_eq!(detect(TrustCli::Antigravity, &screen(&spread)), None);
    assert_eq!(TrustCli::from_cli("agy"), Some(TrustCli::Antigravity));
    assert_eq!(TrustCli::from_cli("codex"), Some(TrustCli::Codex));
    assert_eq!(TrustCli::from_cli("cursor"), None);
}

#[test]
fn a_dialog_whose_folder_is_not_one_path_is_not_a_prompt() {
    let prompt = |folder: &str| format!(
        "Accessing workspace:\r\n{folder}\r\n\r\nDo you trust the contents of this project?\r\n> Yes, I trust this folder\r\n  No, exit\r\n  Navigate - enter Confirm\r\n"
    );
    for folder in [r"D:\w\worker-1", r"\\server\share\worker-1", "/home/me/worker-1"] {
        assert!(detect(TrustCli::Antigravity, &screen(prompt(folder).as_bytes())).is_some(), "{folder}");
    }
    for folder in ["Hello Queen, please run this", &format!("D:\\{}", "a".repeat(MAX_FOLDER_CHARS)), "D:\\a`b"] {
        assert!(detect(TrustCli::Antigravity, &screen(prompt(folder).as_bytes())).is_none(), "{folder}");
    }
}

#[test]
fn a_codex_note_wrapped_by_a_narrow_pane_still_yields_its_root() {
    let narrow = reframe(CODEX_PROMPT, &[(
        "Note: You\u{2019}re in a subdirectory of a Git project. Trusting will apply to the repository root:",
        "Note: You\u{2019}re in a subdirectory of a Git project. Trusting will apply to the",
    )]);
    let mut lines = screen(&narrow);
    let at = lines.iter().position(|line| line.trim().ends_with("Trusting will apply to the")).unwrap();
    lines.insert(at + 1, "  repository root:".into());
    assert_eq!(detect(TrustCli::Codex, &lines).and_then(|prompt| prompt.repo_root), Some(CODEX_ROOT.into()));
}

#[test]
fn moving_the_selection_keeps_the_dialog_detected() {
    let moved = reframe(CODEX_PROMPT, &[
        ("\u{203a} 1. Trust and continue", "  1. Trust and continue"),
        ("  2. Back to Agent Command Center", "\u{203a} 2. Back to Agent Command Center"),
    ]);
    let shown = detect(TrustCli::Codex, &screen(&moved)).expect("still the codex dialog");
    assert!(!shown.trust_highlighted);
    let mut guard = guard(TrustCli::Codex, Instant::now());
    assert!(matches!(guard.observe(&ring(CODEX_PROMPT), 120, 40, Instant::now()), Some(GuardAction::Detected(_))));
    let at = CODEX_PROMPT.len() as u64;
    assert_eq!(guard.observe(&more(at, &moved), 120, 40, Instant::now()), None, "not cleared by a moved selection");
    assert!(guard.is_surfaced());
    let agy = String::from_utf8(AGY_PROMPT.to_vec()).unwrap()
        .replace("> Yes, I trust this folder", "  Yes, I trust this folder").replace("  \u{1b}[38;5;246mNo, exit", "> \u{1b}[38;5;246mNo, exit");
    assert!(detect(TrustCli::Antigravity, &screen(agy.as_bytes())).is_some());
}

#[test]
fn a_codex_question_wrapped_by_a_narrow_pane_is_still_detected() {
    let mut lines = screen(CODEX_PROMPT);
    let at = lines.iter().position(|line| line.trim().starts_with("Trust this folder?")).unwrap();
    let question = lines[at].clone();
    let (head, tail) = question.split_at(question.find(" Codex can").unwrap());
    lines[at] = head.to_string();
    lines.insert(at + 1, format!("  {}", tail.trim()));
    assert!(detect(TrustCli::Codex, &lines).is_some());
}

#[cfg(windows)]
#[test]
fn a_windows_executable_path_names_its_cli() {
    assert_eq!(TrustCli::from_cli(r"C:\bin\codex.exe"), Some(TrustCli::Codex));
}

/// A captured prompt is surfaced once and cleared by the CLI's real response to Enter
/// (pressed by the operator; the guard never writes).
fn surface_flow(cli: TrustCli, prompt: &[u8], after: &[u8]) {
    let t0 = Instant::now();
    let mut guard = guard(cli, t0);
    assert_eq!(guard.observe(&ring(&prompt[..prompt.len() / 3]), 120, 40, t0), None, "partial frame");
    let shown = guard.observe(&ring(prompt), 120, 40, t0 + Duration::from_secs(1));
    assert!(matches!(&shown, Some(GuardAction::Detected(_))), "{shown:?}");
    assert!(guard.is_surfaced());
    assert_eq!(guard.observe(&ring(prompt), 120, 40, t0 + Duration::from_secs(30)), None, "surfaced once");
    let whole = concat(prompt, after);
    let cleared = guard.observe(&ring(&whole), 120, 40, t0 + Duration::from_secs(40));
    assert!(matches!(cleared, Some(GuardAction::Cleared(_))), "{cleared:?}");
    assert!(!guard.is_active(t0 + Duration::from_secs(41)));
    assert_eq!(guard.observe(&ring(&concat(&whole, prompt)), 120, 40, t0 + Duration::from_secs(50)), None,
        "a finished guard reports nothing more");
}

#[test]
fn every_captured_prompt_is_surfaced_once_and_cleared() {
    surface_flow(TrustCli::Antigravity, AGY_PROMPT, AGY_AFTER);
    surface_flow(TrustCli::Codex, CODEX_PROMPT, CODEX_AFTER);
    surface_flow(TrustCli::Claude, CLAUDE_PROMPT, b"\x1b[2J\x1b[H> working\r\n");
}

#[test]
fn a_resize_without_a_repaint_never_clears_a_pending_prompt_early() {
    let t0 = Instant::now();
    let len = AGY_PROMPT.len() as u64;
    let mut guard = guard(TrustCli::Antigravity, t0);
    assert!(matches!(guard.observe(&ring(AGY_PROMPT), 120, 40, t0), Some(GuardAction::Detected(_))));
    let nothing = more(len, b"");
    assert_eq!(guard.observe(&nothing, 100, 30, t0), None, "resized, nothing repainted yet");
    assert_eq!(guard.observe(&nothing, 100, 30, t0 + Duration::from_secs(5)), None, "still nothing new");
    assert_eq!(guard.observe(&more(len, b"\x1b[1;1Hx"), 100, 30, t0 + Duration::from_secs(10)), None,
        "a partial repaint cannot clear it");
    assert!(guard.is_surfaced());
    // The repaint brings the prompt back: it was never gone.
    assert_eq!(guard.observe(&more(len + 7, AGY_PROMPT), 100, 30, t0 + Duration::from_secs(15)), None);
    assert!(guard.is_surfaced());
    // A later full-screen redraw without the prompt is a real answer.
    let at = len * 2 + 7;
    assert!(matches!(guard.observe(&more(at, b"\x1b[2J\x1b[Hdone"), 100, 30, t0 + Duration::from_secs(20)),
        Some(GuardAction::Cleared(_))));
}

#[test]
fn a_resize_whose_own_bytes_redraw_everything_is_complete_at_once() {
    let t0 = Instant::now();
    let len = AGY_PROMPT.len() as u64;
    let mut guard = guard(TrustCli::Antigravity, t0);
    let _ = guard.observe(&ring(AGY_PROMPT), 120, 40, t0);
    assert!(matches!(guard.observe(&more(len, b"\x1b[2J\x1b[Hdone"), 90, 30, t0), Some(GuardAction::Cleared(_))));
}

#[test]
fn a_cli_that_never_repaints_after_a_resize_stays_surfaced() {
    let t0 = Instant::now();
    let len = AGY_PROMPT.len() as u64;
    let mut guard = guard(TrustCli::Antigravity, t0);
    let _ = guard.observe(&ring(AGY_PROMPT), 120, 40, t0);
    let nothing = more(len, b"");
    assert_eq!(guard.observe(&nothing, 100, 30, t0), None);
    assert_eq!(guard.observe(&nothing, 100, 30, t0 + Duration::from_secs(600)), None, "time alone never clears it");
    // Escape-only output (a cursor blink, mode switches) draws nothing.
    assert_eq!(guard.observe(&more(len, b"\x1b[?25l\x1b[?25h\x1b[H"), 100, 30, t0 + Duration::from_secs(601)), None);
    assert!(guard.is_surfaced(), "a notice that stays up hangs nothing");
}

#[test]
fn an_oversized_pty_does_not_reset_the_grid_every_turn() {
    let t0 = Instant::now();
    let mut guard = TrustPromptGuard::new(TrustCli::Antigravity, 2000, 900, t0);
    assert!(matches!(guard.observe(&ring(AGY_PROMPT), 2000, 900, t0), Some(GuardAction::Detected(_))));
    let at = AGY_PROMPT.len() as u64;
    assert_eq!(guard.observe(&more(at, b"\x1b[50;1Htick"), 2000, 900, t0 + Duration::from_secs(5)), None);
    assert!(guard.is_surfaced(), "the prompt is still on the kept grid");
}

#[test]
fn leaving_the_alternate_screen_counts_as_a_full_redraw() {
    assert!(clears_screen(b"\x1b[?1049l"));
    assert!(!clears_screen(b"\x1b[3J"), "scrollback erase leaves the view");
}

#[test]
fn guard_reads_only_new_output_and_recovers_from_eviction() {
    let t0 = Instant::now();
    let mut guard = guard(TrustCli::Antigravity, t0);
    assert_eq!(guard.observe(&ring(&AGY_PROMPT[..1000]), 120, 40, t0), None);
    assert_eq!(guard.fed_to(), 1000);
    let rest = OutputSnapshot { offset_start: 1000, offset_end: AGY_PROMPT.len() as u64, data: AGY_PROMPT[1000..].to_vec() };
    assert!(matches!(guard.observe(&rest, 120, 40, t0), Some(GuardAction::Detected(_))), "incremental feed sees the prompt");

    // Output before 1500 was evicted unseen: the guard restarts from what is left.
    let mut late = TrustPromptGuard::new(TrustCli::Antigravity, 120, 40, t0);
    let _ = late.observe(&ring(&AGY_PROMPT[..1000]), 120, 40, t0);
    let evicted = OutputSnapshot { offset_start: 1500, offset_end: AGY_PROMPT.len() as u64, data: AGY_PROMPT[1500..].to_vec() };
    let _ = late.observe(&evicted, 120, 40, t0);
    assert_eq!(late.fed_to(), AGY_PROMPT.len() as u64);
}

#[test]
fn a_guard_that_saw_no_prompt_stops_after_the_watch_window() {
    let t0 = Instant::now();
    let mut guard = guard(TrustCli::Codex, t0);
    assert!(guard.is_active(t0 + WATCH_WINDOW - Duration::from_secs(1)));
    assert_eq!(guard.observe(&ring(CODEX_PROMPT), 120, 40, t0 + WATCH_WINDOW), None);
    assert!(!guard.is_active(t0));
}
