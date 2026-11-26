//! Detect and answer a CLI's folder-trust prompt on a worker's PTY screen (#466).
//!
//! Prevention (`cli::folder_trust`) pre-trusts the worktree for codex and agy, so this is
//! the fallback for a CLI version that ignores the override, and the only handling for
//! Claude Code, which has no per-process trust override.
//!
//! The prompt texts below come from real captures, kept as fixtures in
//! `trust_prompt_fixtures/`: agy 1.5.18, codex-cli 0.171.0 and Claude Code 2.1.332, each
//! spawned with the adapter's own flags into a fresh, untrusted worktree.
//!
//! Rules, in the order they are checked:
//! 1. The prompt must name the worker's own worktree. Codex also names the repository
//!    root its trust applies to; that must be the worktree's own repository.
//! 2. The highlighted option must be the trust choice, so the accept key is a bare Enter.
//!    Claude Code highlights "No, exit", so its prompt is surfaced and never answered.
//! 3. Accept at most once per spawn. If the prompt is still there or the screen has not
//!    moved after the accept, surface it instead of pressing again.

use std::time::{Duration, Instant};

use super::output_ring::OutputSnapshot;
use super::screen::Screen;
use crate::cli::folder_trust::{git_repo_root, normalize_path};

/// The status reason set while a trust prompt is on screen.
pub const TRUST_PROMPT_STATUS: &str = "trust_prompt";

/// The accept key. Every captured prompt confirms its highlighted option with Enter.
pub const ACCEPT_KEYS: &[u8] = b"\r";

/// Grace after the accept before a still-visible prompt counts as reappeared.
pub const REAPPEAR_GRACE: Duration = Duration::from_secs(3);

/// How long the rendered screen may stay exactly as it was at the accept. Judged on the
/// grid, not the byte count, so a cursor blink or an echoed key is not progress.
pub const UNCHANGED_LIMIT: Duration = Duration::from_secs(10);

/// How long after spawn a guard keeps watching for a first prompt. Codex drew its prompt
/// 90-115 s after spawn in the captures, behind its startup work.
pub const WATCH_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustCli {
    Antigravity,
    Codex,
    Claude,
}

impl TrustCli {
    /// The CLI from an agent's configured `cli` name or its executable.
    pub fn from_cli(cli: &str) -> Option<Self> {
        let stem = std::path::Path::new(cli).file_stem()?.to_str()?.to_ascii_lowercase();
        match stem.as_str() {
            "antigravity" | "agy" => Some(Self::Antigravity),
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

/// A trust prompt read off the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustPrompt {
    pub cli: TrustCli,
    /// The folder the prompt asks about.
    pub folder: String,
    /// The repository root codex says its trust applies to.
    pub repo_root: Option<String>,
    /// Whether the highlighted option is the trust choice.
    pub trust_highlighted: bool,
}

/// Read a trust prompt from rendered screen lines, if one is showing.
pub fn detect(cli: TrustCli, lines: &[String]) -> Option<TrustPrompt> {
    match cli {
        // agy 1.5.18:
        //   Accessing workspace:
        //   D:\...\worker-1
        //   Do you trust the contents of this project?
        //   > Yes, I trust this folder
        //     No, exit
        TrustCli::Antigravity => {
            find(lines, |l| l == "Do you trust the contents of this project?")?;
            let folder = block_after(lines, |l| l == "Accessing workspace:")?;
            let yes = find(lines, |l| l.ends_with("Yes, I trust this folder"))?;
            Some(TrustPrompt {
                cli,
                folder,
                repo_root: None,
                trust_highlighted: lines[yes].trim() == "> Yes, I trust this folder",
            })
        }
        // codex-cli 0.171.0:
        //   Folder access
        //   D:\...\worker-1
        //   Note: You're in a subdirectory of a Git project. Trusting will apply to the repository root:
        //   d:\...\repo
        //   Trust this folder? Codex can read, edit, and run files here, ...
        //   › 1. Trust and continue
        //     2. Back to Agent Command Center
        TrustCli::Codex => {
            find(lines, |l| l.starts_with("Trust this folder? Codex can read, edit, and run files here"))?;
            let folder = block_after(lines, |l| l == "Folder access")?;
            let repo_root = block_after(lines, |l| l.ends_with("Trusting will apply to the repository root:"));
            let trust = find(lines, |l| l.ends_with("1. Trust and continue"))?;
            Some(TrustPrompt {
                cli,
                folder,
                repo_root,
                trust_highlighted: lines[trust].trim() == "› 1. Trust and continue",
            })
        }
        // Claude Code 2.1.332:
        //   Accessing workspace:
        //   D:\...\worker-1
        //   Quick safety check: Is this a project you created or one you trust? ...
        //   ❯ No, exit
        //     Yes, I trust this folder
        TrustCli::Claude => {
            find(lines, |l| l.contains("Is this a project you created or one you trust?"))?;
            let folder = block_after(lines, |l| l == "Accessing workspace:")?;
            let yes = find(lines, |l| l.ends_with("Yes, I trust this folder"))?;
            Some(TrustPrompt {
                cli,
                folder,
                repo_root: None,
                trust_highlighted: lines[yes].trim() == "❯ Yes, I trust this folder",
            })
        }
    }
}

fn find(lines: &[String], matches: impl Fn(&str) -> bool) -> Option<usize> {
    lines.iter().position(|line| matches(line.trim()))
}

/// The text after an anchor line up to the next blank line, joined, so a long path the
/// CLI wrapped onto two rows reads back as one.
fn block_after(lines: &[String], anchor: impl Fn(&str) -> bool) -> Option<String> {
    let start = find(lines, anchor)? + 1;
    let block: String = lines[start..]
        .iter()
        .map(|line| line.trim())
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect();
    (!block.is_empty()).then_some(block)
}

/// Why a prompt is not answered automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The prompt names a folder other than the worker's worktree.
    ForeignPath,
    /// Codex would apply the trust to a repository root that is not the worktree's own.
    ForeignRepoRoot,
    /// The highlighted option is not the trust choice (Claude Code defaults to "No").
    DefaultIsNotTrust,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::ForeignPath => "foreign_path",
            Self::ForeignRepoRoot => "foreign_repo_root",
            Self::DefaultIsNotTrust => "default_is_not_trust",
        }
    }
}

/// Whether `prompt` may be answered for a worker whose worktree is `cwd`.
pub fn check(prompt: &TrustPrompt, cwd: &str) -> Result<(), Refusal> {
    if normalize_path(&prompt.folder) != normalize_path(cwd) {
        return Err(Refusal::ForeignPath);
    }
    if let Some(root) = &prompt.repo_root {
        let root = normalize_path(root);
        let own = git_repo_root(std::path::Path::new(cwd))
            .map(|own| normalize_path(&own.to_string_lossy()));
        if root != normalize_path(cwd) && own.as_deref() != Some(root.as_str()) {
            return Err(Refusal::ForeignRepoRoot);
        }
    }
    if !prompt.trust_highlighted {
        return Err(Refusal::DefaultIsNotTrust);
    }
    Ok(())
}

/// Why an accepted prompt was handed to the Queen and operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// The screen changed after the accept but the prompt is still on it.
    Reappeared,
    /// The rendered screen is exactly what it was at the accept.
    Unchanged,
    /// The accept keystroke could not be written.
    WriteFailed,
}

impl Escalation {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Reappeared => "reappeared",
            Self::Unchanged => "screen_unchanged",
            Self::WriteFailed => "write_failed",
        }
    }
}

/// What one observation asks the caller to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardAction {
    /// Write [`ACCEPT_KEYS`], then report back with [`TrustPromptGuard::accept_written`].
    Accept(TrustPrompt),
    /// The prompt is up but must not be answered: surface it.
    Refused(TrustPrompt, Refusal),
    /// The accepted prompt is gone.
    Cleared(TrustPrompt),
    /// The accepted prompt did not go away: surface it.
    Escalated(TrustPrompt, Escalation),
}

#[derive(Debug, Clone)]
enum GuardState {
    Watching,
    Accepted { prompt: TrustPrompt, at_screen: Vec<String>, at: Instant },
    Done,
}

/// Per-spawn trust-prompt state: a screen fed from the agent's output ring, plus the
/// once-only accept bookkeeping.
#[derive(Debug, Clone)]
pub struct TrustPromptGuard {
    cli: TrustCli,
    cwd: Option<String>,
    started: Instant,
    screen: Screen,
    fed_to: Option<u64>,
    state: GuardState,
}

impl TrustPromptGuard {
    pub fn new(cli: TrustCli, cwd: Option<String>, cols: u16, rows: u16, started: Instant) -> Self {
        Self {
            cli,
            cwd,
            started,
            screen: Screen::new(cols, rows),
            fed_to: None,
            state: GuardState::Watching,
        }
    }

    /// Whether the guard still needs output. A finished guard never acts again, which is
    /// what makes the accept once-per-spawn.
    pub fn is_active(&self, now: Instant) -> bool {
        match self.state {
            GuardState::Done => false,
            GuardState::Watching => now.duration_since(self.started) < WATCH_WINDOW,
            GuardState::Accepted { .. } => true,
        }
    }

    /// The absolute offset the guard has rendered up to; pass it to the output source.
    pub fn fed_to(&self) -> u64 {
        self.fed_to.unwrap_or(0)
    }

    /// Render the output in `snapshot` that is new since the last call, at the PTY's
    /// current size, and decide what to do.
    pub fn observe(&mut self, snapshot: &OutputSnapshot, cols: u16, rows: u16, now: Instant) -> Option<GuardAction> {
        if !self.is_active(now) {
            if matches!(self.state, GuardState::Watching) {
                self.state = GuardState::Done;
            }
            return None;
        }
        self.feed(snapshot, cols, rows);
        let lines = self.screen.lines();
        let shown = detect(self.cli, &lines);
        match std::mem::replace(&mut self.state, GuardState::Done) {
            GuardState::Watching => {
                let Some(prompt) = shown else {
                    self.state = GuardState::Watching;
                    return None;
                };
                let Some(cwd) = self.cwd.as_deref() else {
                    return Some(GuardAction::Refused(prompt, Refusal::ForeignPath));
                };
                match check(&prompt, cwd) {
                    Ok(()) => {
                        self.state = GuardState::Accepted { prompt: prompt.clone(), at_screen: lines, at: now };
                        Some(GuardAction::Accept(prompt))
                    }
                    Err(refusal) => Some(GuardAction::Refused(prompt, refusal)),
                }
            }
            GuardState::Accepted { prompt, at_screen, at } => {
                let waited = now.duration_since(at);
                if shown.is_none() {
                    return Some(GuardAction::Cleared(prompt));
                }
                if lines == at_screen {
                    if waited >= UNCHANGED_LIMIT {
                        return Some(GuardAction::Escalated(prompt, Escalation::Unchanged));
                    }
                } else if waited >= REAPPEAR_GRACE {
                    return Some(GuardAction::Escalated(prompt, Escalation::Reappeared));
                }
                self.state = GuardState::Accepted { prompt, at_screen, at };
                None
            }
            GuardState::Done => None,
        }
    }

    /// Record the outcome of writing the accept keys. A failed write is escalated.
    pub fn accept_written(&mut self, ok: bool) -> Option<GuardAction> {
        if ok {
            return None;
        }
        match std::mem::replace(&mut self.state, GuardState::Done) {
            GuardState::Accepted { prompt, .. } => Some(GuardAction::Escalated(prompt, Escalation::WriteFailed)),
            other => {
                self.state = other;
                None
            }
        }
    }

    fn feed(&mut self, snapshot: &OutputSnapshot, cols: u16, rows: u16) {
        // A resize makes ConPTY repaint the whole viewport, so a fresh grid at the new
        // size is filled again by the next frame.
        if self.screen.size() != (cols, rows) {
            self.screen = Screen::new(cols, rows);
        }
        let from = match self.fed_to {
            // Output we never saw was evicted: start again from what is retained.
            Some(fed) if fed < snapshot.offset_start => {
                self.screen = Screen::new(cols, rows);
                snapshot.offset_start
            }
            Some(fed) => fed,
            None => snapshot.offset_start,
        };
        let skip = from.saturating_sub(snapshot.offset_start) as usize;
        if let Some(new) = snapshot.data.get(skip..) {
            self.screen.feed(new);
        }
        self.fed_to = Some(snapshot.offset_end.max(from));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGY_PROMPT: &[u8] = include_bytes!("trust_prompt_fixtures/agy-1.5.18-prompt.bin");
    const AGY_AFTER: &[u8] = include_bytes!("trust_prompt_fixtures/agy-1.5.18-after-accept.bin");
    const CODEX_PROMPT: &[u8] = include_bytes!("trust_prompt_fixtures/codex-0.171.0-prompt.bin");
    const CODEX_AFTER: &[u8] = include_bytes!("trust_prompt_fixtures/codex-0.171.0-after-accept.bin");
    const CLAUDE_PROMPT: &[u8] = include_bytes!("trust_prompt_fixtures/claude-2.1.332-prompt.bin");

    // The fixtures were captured at 120x40 in these scratch worktrees.
    const AGY_CWD: &str = r"D:\hm466-scratch\repo-agy-acc2";
    const CODEX_CWD: &str = r"D:\hm466-scratch\repo-codex2-acc";
    const CODEX_ROOT: &str = r"d:\hm466-scratch\repo-codex2";
    const CLAUDE_CWD: &str = r"D:\hm466-scratch\repo-claude-wt1";

    fn screen(bytes: &[u8]) -> Vec<String> {
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

    /// The codex capture with its repository root swapped for `root`, so a test never
    /// depends on whether the scratch repository still exists on this disk.
    fn codex_prompt_with_root(root: &str) -> Vec<u8> {
        let text = String::from_utf8(CODEX_PROMPT.to_vec()).unwrap();
        assert_eq!(text.matches(CODEX_ROOT).count(), 1);
        text.replace(CODEX_ROOT, root).into_bytes()
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
    fn the_screen_after_each_accept_shows_no_prompt_and_the_cli_working() {
        let agy = screen(&concat(AGY_PROMPT, AGY_AFTER));
        assert_eq!(detect(TrustCli::Antigravity, &agy), None);
        assert!(agy.iter().any(|line| line.trim() == "READY"), "{agy:#?}");
        let codex = screen(&concat(CODEX_PROMPT, CODEX_AFTER));
        assert_eq!(detect(TrustCli::Codex, &codex), None);
        assert!(codex.iter().any(|line| line.trim() == "• READY"), "{codex:#?}");
    }

    #[test]
    fn prompts_are_specific_to_their_cli_and_plain_output_is_not_a_prompt() {
        assert_eq!(detect(TrustCli::Codex, &screen(AGY_PROMPT)), None);
        assert_eq!(detect(TrustCli::Antigravity, &screen(CODEX_PROMPT)), None);
        assert_eq!(detect(TrustCli::Codex, &screen(b"Trust this folder?\r\n")), None);
        assert_eq!(TrustCli::from_cli("agy"), Some(TrustCli::Antigravity));
        assert_eq!(TrustCli::from_cli(r"C:\bin\codex.exe"), Some(TrustCli::Codex));
        assert_eq!(TrustCli::from_cli("cursor"), None);
    }

    /// Walks a guard through a captured prompt and the CLI's real response to Enter.
    fn accept_flow(cli: TrustCli, cwd: &str, prompt: &[u8], after: &[u8]) {
        let t0 = Instant::now();
        let mut guard = TrustPromptGuard::new(cli, Some(cwd.into()), 120, 40, t0);
        assert_eq!(guard.observe(&ring(&prompt[..prompt.len() / 3]), 120, 40, t0), None, "partial frame");

        let shown = guard.observe(&ring(prompt), 120, 40, t0 + Duration::from_secs(1));
        assert!(matches!(&shown, Some(GuardAction::Accept(p)) if normalize_path(&p.folder) == normalize_path(cwd)), "{shown:?}");
        assert_eq!(guard.accept_written(true), None);
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
    fn agy_prompt_is_detected_accepted_once_and_cleared() {
        accept_flow(TrustCli::Antigravity, AGY_CWD, AGY_PROMPT, AGY_AFTER);
    }

    #[test]
    fn codex_prompt_is_detected_accepted_once_and_cleared() {
        let t0 = Instant::now();
        let mut guard = TrustPromptGuard::new(TrustCli::Codex, Some(CODEX_CWD.into()), 120, 40, t0);
        // A root that is not the worktree's own repository is refused...
        let foreign = codex_prompt_with_root(r"d:\hm466-fixture\not-this-repo");
        let refused = guard.observe(&ring(&foreign), 120, 40, t0);
        assert!(matches!(refused, Some(GuardAction::Refused(_, Refusal::ForeignRepoRoot))), "{refused:?}");
        // ...and one that is the worktree's own is accepted. On disk that is the main
        // checkout (see `codex_repo_root_must_be_the_worktrees_own_main_checkout`).
        accept_flow(TrustCli::Codex, CODEX_CWD, &codex_prompt_with_root(CODEX_CWD), CODEX_AFTER);
    }

    #[test]
    fn codex_repo_root_must_be_the_worktrees_own_main_checkout() {
        // A linked worktree on disk, as a hive lays one out.
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let worktree = repo.join(".hive-manager").join("worktrees").join("s1").join("worker-1");
        let admin = repo.join(".git").join("worktrees").join("worker-1");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let cwd = worktree.to_string_lossy().into_owned();
        let shown = |root: &std::path::Path| TrustPrompt {
            cli: TrustCli::Codex,
            folder: cwd.clone(),
            repo_root: Some(root.to_string_lossy().into_owned()),
            trust_highlighted: true,
        };
        assert_eq!(check(&shown(&repo), &cwd), Ok(()));
        let other = temp.path().join("other-repo");
        assert_eq!(check(&shown(&other), &cwd), Err(Refusal::ForeignRepoRoot));
    }

    #[test]
    fn a_prompt_naming_a_foreign_path_is_never_accepted() {
        for (cli, prompt) in [(TrustCli::Antigravity, AGY_PROMPT), (TrustCli::Codex, CODEX_PROMPT)] {
            let t0 = Instant::now();
            let mut guard = TrustPromptGuard::new(cli, Some(r"D:\hm466-scratch\someone-else".into()), 120, 40, t0);
            let action = guard.observe(&ring(prompt), 120, 40, t0);
            assert!(matches!(action, Some(GuardAction::Refused(_, Refusal::ForeignPath))), "{cli:?}: {action:?}");
            assert!(!guard.is_active(t0), "refused once, then surfaced, never retried");
        }
        let mut no_cwd = TrustPromptGuard::new(TrustCli::Antigravity, None, 120, 40, Instant::now());
        assert!(matches!(no_cwd.observe(&ring(AGY_PROMPT), 120, 40, Instant::now()),
            Some(GuardAction::Refused(_, Refusal::ForeignPath))));
    }

    #[test]
    fn claude_prompt_defaulting_to_no_is_surfaced_not_answered() {
        let mut guard = TrustPromptGuard::new(TrustCli::Claude, Some(CLAUDE_CWD.into()), 120, 40, Instant::now());
        let action = guard.observe(&ring(CLAUDE_PROMPT), 120, 40, Instant::now());
        assert!(matches!(action, Some(GuardAction::Refused(_, Refusal::DefaultIsNotTrust))), "{action:?}");
    }

    #[test]
    fn an_accept_that_changes_nothing_or_brings_the_prompt_back_is_escalated() {
        let t0 = Instant::now();
        let mut silent = TrustPromptGuard::new(TrustCli::Antigravity, Some(AGY_CWD.into()), 120, 40, t0);
        assert!(matches!(silent.observe(&ring(AGY_PROMPT), 120, 40, t0), Some(GuardAction::Accept(_))));
        assert_eq!(silent.observe(&ring(AGY_PROMPT), 120, 40, t0 + Duration::from_secs(5)), None, "still waiting");
        let action = silent.observe(&ring(AGY_PROMPT), 120, 40, t0 + UNCHANGED_LIMIT);
        assert!(matches!(action, Some(GuardAction::Escalated(_, Escalation::Unchanged))), "{action:?}");
        assert!(!silent.is_active(t0 + UNCHANGED_LIMIT));

        let mut back = TrustPromptGuard::new(TrustCli::Antigravity, Some(AGY_CWD.into()), 120, 40, t0);
        assert!(matches!(back.observe(&ring(AGY_PROMPT), 120, 40, t0), Some(GuardAction::Accept(_))));
        // The CLI drew something new, yet the prompt is still up.
        let redrawn = concat(AGY_PROMPT, b"\x1b[30;1Hstill asking");
        assert_eq!(back.observe(&ring(&redrawn), 120, 40, t0 + Duration::from_secs(1)), None, "inside the grace");
        let action = back.observe(&ring(&redrawn), 120, 40, t0 + REAPPEAR_GRACE);
        assert!(matches!(action, Some(GuardAction::Escalated(_, Escalation::Reappeared))), "{action:?}");

        let mut failed = TrustPromptGuard::new(TrustCli::Antigravity, Some(AGY_CWD.into()), 120, 40, t0);
        assert!(matches!(failed.observe(&ring(AGY_PROMPT), 120, 40, t0), Some(GuardAction::Accept(_))));
        assert!(matches!(failed.accept_written(false), Some(GuardAction::Escalated(_, Escalation::WriteFailed))));
    }

    #[test]
    fn guard_reads_only_new_output_and_recovers_from_eviction() {
        let t0 = Instant::now();
        let mut guard = TrustPromptGuard::new(TrustCli::Antigravity, Some(AGY_CWD.into()), 120, 40, t0);
        let head = &AGY_PROMPT[..1000];
        assert_eq!(guard.observe(&ring(head), 120, 40, t0), None);
        assert_eq!(guard.fed_to(), 1000);
        // The ring evicted everything before 1500: the guard restarts from what is left,
        // which here is a partial frame, and waits for the next full one.
        let evicted = OutputSnapshot { offset_start: 1500, offset_end: AGY_PROMPT.len() as u64, data: AGY_PROMPT[1500..].to_vec() };
        let _ = guard.observe(&evicted, 120, 40, t0);
        assert_eq!(guard.fed_to(), AGY_PROMPT.len() as u64);
        let mut fresh = TrustPromptGuard::new(TrustCli::Antigravity, Some(AGY_CWD.into()), 120, 40, t0);
        let start = ring(&AGY_PROMPT[..1000]);
        assert_eq!(fresh.observe(&start, 120, 40, t0), None);
        let rest = OutputSnapshot { offset_start: 0, offset_end: AGY_PROMPT.len() as u64, data: AGY_PROMPT.to_vec() };
        assert!(matches!(fresh.observe(&rest, 120, 40, t0), Some(GuardAction::Accept(_))), "incremental feed sees the prompt");
    }

    #[test]
    fn a_guard_that_saw_no_prompt_stops_after_the_watch_window() {
        let t0 = Instant::now();
        let mut guard = TrustPromptGuard::new(TrustCli::Codex, Some(CODEX_CWD.into()), 120, 40, t0);
        assert!(guard.is_active(t0 + WATCH_WINDOW - Duration::from_secs(1)));
        assert_eq!(guard.observe(&ring(CODEX_PROMPT), 120, 40, t0 + WATCH_WINDOW), None);
        assert!(!guard.is_active(t0));
    }
}
