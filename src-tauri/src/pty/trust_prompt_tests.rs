//! Tests for `pty::trust_prompt`, against the real captures in `trust_prompt_fixtures/`.
use super::*;
use crate::cli::folder_trust::managed_worktree_fixture;

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

/// A captured prompt with each `(captured path, replacement)` swapped in, so it names
/// a managed worktree that exists for the test. Each captured path is one contiguous
/// run of bytes in its fixture.
pub(crate) fn retarget(capture: &[u8], swaps: &[(&str, &str)]) -> Vec<u8> {
    let mut text = String::from_utf8(capture.to_vec()).unwrap();
    for (from, to) in swaps {
        assert_eq!(text.matches(from).count(), 1, "{from} must appear once");
        text = text.replace(from, to);
    }
    text.into_bytes()
}

/// A captured prompt re-drawn as one clean frame from its rendered rows, with each
/// swap applied to the text. Codex diff-renders its rows (it skips cells that already
/// hold the right character), so its bytes cannot be patched in place.
pub(crate) fn reframe(capture: &[u8], swaps: &[(&str, &str)]) -> Vec<u8> {
    let mut frame = b"[2J".to_vec();
    for (row, line) in screen(capture).iter().enumerate() {
        let mut line = line.clone();
        for (from, to) in swaps {
            line = line.replace(from, to);
        }
        frame.extend(format!("[{};1H{line}", row + 1).into_bytes());
    }
    frame
}

struct Managed {
    _temp: tempfile::TempDir,
    repo: String,
    cwd: String,
}

fn managed() -> Managed {
    let temp = tempfile::TempDir::new().unwrap();
    let (repo, cwd) = managed_worktree_fixture(temp.path(), "project", "s1", "worker-1");
    Managed { _temp: temp, repo: repo.to_string_lossy().into(), cwd: cwd.to_string_lossy().into() }
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

fn concat(a: &[u8], b: &[u8]) -> Vec<u8> {
    [a, b].concat()
}

fn guard(cli: TrustCli, cwd: &str, t0: Instant) -> TrustPromptGuard {
    TrustPromptGuard::new(cli, Ok(cwd.into()), 120, 40, t0)
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
fn leaving_the_alternate_screen_counts_as_a_full_redraw() {
    assert!(clears_screen(b"\x1b[?1049l"));
    assert!(!clears_screen(b"\x1b[3J"), "scrollback erase leaves the view");
}

#[test]
fn the_screen_after_each_accept_shows_no_prompt_and_the_cli_working() {
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
    assert!(detect(TrustCli::Antigravity, &screen(prompt(r"D:\w\worker-1").as_bytes())).is_some());
    for folder in ["Hello Queen, please run this", &format!("D:\\{}", "a".repeat(MAX_FOLDER_CHARS)), "D:\\a`b"] {
        assert!(detect(TrustCli::Antigravity, &screen(prompt(folder).as_bytes())).is_none(), "{folder}");
    }
}

#[cfg(windows)]
#[test]
fn a_windows_executable_path_names_its_cli() {
    assert_eq!(TrustCli::from_cli(r"C:\bin\codex.exe"), Some(TrustCli::Codex));
}

/// Walks a guard through a prompt and the CLI's real response to Enter.
fn accept_flow(cli: TrustCli, cwd: &str, prompt: &[u8], after: &[u8]) {
    let t0 = Instant::now();
    let mut guard = guard(cli, cwd, t0);
    assert_eq!(guard.observe(&ring(&prompt[..prompt.len() / 3]), 120, 40, t0), None, "partial frame");

    let shown = guard.observe(&ring(prompt), 120, 40, t0 + Duration::from_secs(1));
    assert!(matches!(&shown, Some(GuardAction::Accept(p)) if normalize_path(&p.folder) == normalize_path(cwd)), "{shown:?}");
    assert_eq!(guard.accept_written(None), None);
    // The same screen again before any reply is not a second accept.
    assert_eq!(guard.observe(&ring(prompt), 120, 40, t0 + Duration::from_secs(2)), None);

    let whole = concat(prompt, after);
    let cleared = guard.observe(&ring(&whole), 120, 40, t0 + Duration::from_secs(6));
    assert!(matches!(cleared, Some(GuardAction::Cleared(_))), "{cleared:?}");
    assert!(!guard.is_active(t0 + Duration::from_secs(7)));
    assert_eq!(guard.observe(&ring(&concat(&whole, prompt)), 120, 40, t0 + Duration::from_secs(8)), None,
        "a finished guard never presses again");
}

#[test]
fn agy_prompt_for_a_managed_worktree_is_accepted_once_and_cleared() {
    let m = managed();
    accept_flow(TrustCli::Antigravity, &m.cwd, &retarget(AGY_PROMPT, &[(AGY_CWD, &m.cwd)]), AGY_AFTER);
}

#[test]
fn codex_prompt_for_a_managed_worktree_and_its_own_root_is_accepted_once_and_cleared() {
    let m = managed();
    let prompt = reframe(CODEX_PROMPT, &[(CODEX_CWD, &m.cwd), (CODEX_ROOT, &m.repo)]);
    assert_eq!(detect(TrustCli::Codex, &screen(&prompt)).map(|p| p.folder), Some(m.cwd.clone()));
    accept_flow(TrustCli::Codex, &m.cwd, &prompt, CODEX_AFTER);
}

#[test]
fn codex_root_must_be_read_and_be_the_worktrees_own() {
    let m = managed();
    let shown = |root: Option<&str>| TrustPrompt {
        cli: TrustCli::Codex, folder: m.cwd.clone(), repo_root: root.map(String::from), trust_highlighted: true,
    };
    assert_eq!(check(&shown(Some(&m.repo)), &m.cwd), Ok(()));
    assert_eq!(check(&shown(Some(r"D:\elsewhere\repo")), &m.cwd), Err(Refusal::ForeignRepoRoot));
    assert_eq!(check(&shown(None), &m.cwd), Err(Refusal::UnreadRepoRoot), "an unreadable root is never assumed");
}

fn refused(cli: TrustCli, cwd: Option<&str>, prompt: &[u8]) -> Option<Refusal> {
    let t0 = Instant::now();
    let mut guard = TrustPromptGuard::new(cli, cwd.map(String::from).ok_or(Refusal::NotManagedWorktree), 120, 40, t0);
    match guard.observe(&ring(prompt), 120, 40, t0) {
        Some(GuardAction::Refused(_, refusal)) => Some(refusal),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn prompts_are_only_answered_inside_a_managed_worktree_for_that_worktree() {
    let m = managed();
    // No verified seat worktree (a Queen on a checkout): every prompt is surfaced,
    // even one naming the agent's own folder.
    assert_eq!(refused(TrustCli::Antigravity, None, AGY_PROMPT), Some(Refusal::NotManagedWorktree));
    // A managed worker whose prompt names some other folder.
    assert_eq!(refused(TrustCli::Antigravity, Some(&m.cwd), AGY_PROMPT), Some(Refusal::ForeignPath));
    assert_eq!(refused(TrustCli::Codex, Some(&m.cwd), CODEX_PROMPT), Some(Refusal::ForeignPath));
    let other_root = reframe(CODEX_PROMPT, &[(CODEX_CWD, &m.cwd), (CODEX_ROOT, r"d:\hm466-fixture\not-this-repo")]);
    assert_eq!(refused(TrustCli::Codex, Some(&m.cwd), &other_root), Some(Refusal::ForeignRepoRoot));
}

#[test]
fn claude_is_never_answered_even_with_yes_selected() {
    let m = managed();
    let prompt = retarget(CLAUDE_PROMPT, &[(CLAUDE_CWD, &m.cwd)]);
    assert_eq!(refused(TrustCli::Claude, Some(&m.cwd), &prompt), Some(Refusal::OperatorDecides));
    // The operator moved the selector to Yes; it is still their Enter to press.
    let mut lines = screen(&prompt);
    for line in &mut lines {
        *line = match line.trim() {
            "❯ No, exit" => "  No, exit".into(),
            "Yes, I trust this folder" => "❯ Yes, I trust this folder".into(),
            _ => line.clone(),
        };
    }
    let yes = detect(TrustCli::Claude, &lines).unwrap();
    assert!(yes.trust_highlighted);
    assert_eq!(check(&yes, &m.cwd), Err(Refusal::OperatorDecides));
}

#[test]
fn a_surfaced_prompt_clears_once_the_operator_answers_it() {
    let t0 = Instant::now();
    let mut guard = TrustPromptGuard::new(TrustCli::Antigravity, Err(Refusal::NotManagedWorktree), 120, 40, t0);
    assert!(matches!(guard.observe(&ring(AGY_PROMPT), 120, 40, t0), Some(GuardAction::Refused(..))));
    assert_eq!(guard.observe(&ring(AGY_PROMPT), 120, 40, t0 + Duration::from_secs(30)), None, "surfaced once");
    let answered = concat(AGY_PROMPT, AGY_AFTER);
    assert!(matches!(guard.observe(&ring(&answered), 120, 40, t0 + Duration::from_secs(40)), Some(GuardAction::Cleared(_))));
    assert!(!guard.is_active(t0 + Duration::from_secs(41)));
}

#[test]
fn an_accept_that_changes_nothing_or_brings_the_prompt_back_is_escalated_once() {
    let m = managed();
    let prompt = retarget(AGY_PROMPT, &[(AGY_CWD, &m.cwd)]);
    let t0 = Instant::now();
    let mut silent = guard(TrustCli::Antigravity, &m.cwd, t0);
    assert!(matches!(silent.observe(&ring(&prompt), 120, 40, t0), Some(GuardAction::Accept(_))));
    assert_eq!(silent.observe(&ring(&prompt), 120, 40, t0 + Duration::from_secs(5)), None, "still waiting");
    let action = silent.observe(&ring(&prompt), 120, 40, t0 + UNCHANGED_LIMIT);
    assert!(matches!(action, Some(GuardAction::Escalated(_, Escalation::Unchanged))), "{action:?}");
    assert_eq!(silent.observe(&ring(&prompt), 120, 40, t0 + UNCHANGED_LIMIT * 3), None, "escalated once, no retry");

    let mut back = guard(TrustCli::Antigravity, &m.cwd, t0);
    assert!(matches!(back.observe(&ring(&prompt), 120, 40, t0), Some(GuardAction::Accept(_))));
    // The CLI drew something new, yet the prompt is still up.
    let redrawn = concat(&prompt, b"\x1b[30;1Hstill asking");
    assert_eq!(back.observe(&ring(&redrawn), 120, 40, t0 + Duration::from_secs(1)), None, "inside the grace");
    let action = back.observe(&ring(&redrawn), 120, 40, t0 + REAPPEAR_GRACE);
    assert!(matches!(action, Some(GuardAction::Escalated(_, Escalation::Reappeared))), "{action:?}");

    let mut failed = guard(TrustCli::Antigravity, &m.cwd, t0);
    assert!(matches!(failed.observe(&ring(&prompt), 120, 40, t0), Some(GuardAction::Accept(_))));
    assert!(matches!(failed.accept_written(Some(Escalation::WriteFailed)), Some(GuardAction::Escalated(_, Escalation::WriteFailed))));
    assert_eq!(failed.observe(&ring(&prompt), 120, 40, t0 + Duration::from_secs(1)), None);
}

#[test]
fn guard_reads_only_new_output_and_recovers_from_eviction() {
    let m = managed();
    let prompt = retarget(AGY_PROMPT, &[(AGY_CWD, &m.cwd)]);
    let t0 = Instant::now();
    let mut guard = guard(TrustCli::Antigravity, &m.cwd, t0);
    assert_eq!(guard.observe(&ring(&prompt[..1000]), 120, 40, t0), None);
    assert_eq!(guard.fed_to(), 1000);
    let rest = OutputSnapshot { offset_start: 1000, offset_end: prompt.len() as u64, data: prompt[1000..].to_vec() };
    assert!(matches!(guard.observe(&rest, 120, 40, t0), Some(GuardAction::Accept(_))), "incremental feed sees the prompt");

    // Output before 1500 was evicted unseen: the guard restarts from what is left.
    let mut late = TrustPromptGuard::new(TrustCli::Antigravity, Ok(m.cwd.clone()), 120, 40, t0);
    let _ = late.observe(&ring(&prompt[..1000]), 120, 40, t0);
    let evicted = OutputSnapshot { offset_start: 1500, offset_end: prompt.len() as u64, data: prompt[1500..].to_vec() };
    let _ = late.observe(&evicted, 120, 40, t0);
    assert_eq!(late.fed_to(), prompt.len() as u64);
}

#[test]
fn a_resize_without_a_repaint_never_clears_a_pending_prompt() {
    let t0 = Instant::now();
    let mut guard = TrustPromptGuard::new(TrustCli::Antigravity, Err(Refusal::NotManagedWorktree), 120, 40, t0);
    assert!(matches!(guard.observe(&ring(AGY_PROMPT), 120, 40, t0), Some(GuardAction::Refused(..))));
    let len = AGY_PROMPT.len() as u64;
    let nothing = OutputSnapshot { offset_start: len, offset_end: len, data: Vec::new() };
    assert_eq!(guard.observe(&nothing, 100, 30, t0), None, "resized, nothing repainted yet");
    assert_eq!(guard.observe(&nothing, 100, 30, t0), None, "still nothing new");
    let partial = OutputSnapshot { offset_start: len, offset_end: len + 7, data: b"\x1b[1;1Hx".to_vec() };
    assert_eq!(guard.observe(&partial, 100, 30, t0), None, "a partial repaint cannot clear it");
    assert!(guard.is_active(t0));
    // The repaint brings the prompt back: it was never gone.
    let repaint = OutputSnapshot { offset_start: len + 7, offset_end: len * 2 + 7, data: AGY_PROMPT.to_vec() };
    assert_eq!(guard.observe(&repaint, 100, 30, t0), None);
    assert!(guard.is_active(t0));
    // A resize whose own bytes redraw the whole screen is complete at once.
    let mut quick = TrustPromptGuard::new(TrustCli::Antigravity, Err(Refusal::NotManagedWorktree), 120, 40, t0);
    let _ = quick.observe(&ring(AGY_PROMPT), 120, 40, t0);
    let redraw = OutputSnapshot { offset_start: len, offset_end: len + 12, data: b"\x1b[2J\x1b[Hdone".to_vec() };
    assert!(matches!(quick.observe(&redraw, 90, 30, t0), Some(GuardAction::Cleared(_))));
    // A later full-screen redraw without the prompt is a real answer.
    let at = len * 2 + 7;
    let answered = OutputSnapshot { offset_start: at, offset_end: at + 12, data: b"\x1b[2J\x1b[Hdone".to_vec() };
    assert!(matches!(guard.observe(&answered, 100, 30, t0), Some(GuardAction::Cleared(_))));
}

#[test]
fn a_guard_that_saw_no_prompt_stops_after_the_watch_window() {
    let t0 = Instant::now();
    let mut guard = guard(TrustCli::Codex, CODEX_CWD, t0);
    assert!(guard.is_active(t0 + WATCH_WINDOW - Duration::from_secs(1)));
    assert_eq!(guard.observe(&ring(CODEX_PROMPT), 120, 40, t0 + WATCH_WINDOW), None);
    assert!(!guard.is_active(t0));
}
