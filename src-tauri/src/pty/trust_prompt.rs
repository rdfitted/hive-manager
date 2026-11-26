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

/// Turns of new output that must follow a grid reset before an absent prompt counts as
/// gone, unless the output clears the whole screen first.
const REBUILD_TURNS: u8 = 2;

/// Whether `bytes` redraw the whole screen from blank: erase display, full reset, or a
/// switch into or out of the alternate screen (agy leaves its trust dialog that way).
fn clears_screen(bytes: &[u8]) -> bool {
    [
        b"\x1b[2J".as_slice(), b"\x1bc", b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?1047h", b"\x1b[?1047l",
        b"\x1b[?47h", b"\x1b[?47l",
    ]
        .iter()
        .any(|needle| bytes.windows(needle.len()).any(|window| window == *needle))
}

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

/// Longest span, in rows, a dialog may take from its first line to its footer. The
/// captured dialogs take 8-13 rows; text scattered further apart is not one dialog.
pub const DIALOG_SPAN: usize = 24;

/// Read a trust prompt from rendered screen lines, if one is showing.
///
/// Every line of the captured dialog must appear in order, from its title to its key
/// hint footer, within [`DIALOG_SPAN`] rows. Loose copies of the same words elsewhere
/// on the screen (scrollback, a file the agent printed) do not make a dialog.
pub fn detect(cli: TrustCli, lines: &[String]) -> Option<TrustPrompt> {
    match cli {
        // agy 1.5.18:
        //   Accessing workspace:
        //   D:\...\worker-1
        //   Do you trust the contents of this project?
        //   Antigravity CLI requires permission to read, edit, and execute files here.
        //   > Yes, I trust this folder
        //     No, exit
        //     ↑/↓ Navigate · enter Confirm
        TrustCli::Antigravity => {
            let at = ordered(lines, &[
                &|l| l == "Accessing workspace:",
                &|l| l == "Do you trust the contents of this project?",
                &|l| l.ends_with("Yes, I trust this folder"),
                &|l| l.ends_with("No, exit"),
                &|l| l.contains("Navigate") && l.ends_with("enter Confirm"),
            ])?;
            Some(TrustPrompt {
                cli,
                folder: block_between(lines, at[0], at[1])?,
                repo_root: None,
                trust_highlighted: lines[at[2]].trim() == "> Yes, I trust this folder",
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
        //     enter continue · esc back
        TrustCli::Codex => {
            let at = ordered(lines, &[
                &|l| l == "Folder access",
                &|l| l.starts_with("Trust this folder? Codex can read, edit, and run files here"),
                &|l| l.ends_with("1. Trust and continue"),
                &|l| l.starts_with("2. "),
                &|l| l.starts_with("enter continue"),
            ])?;
            // Ends with "repository root:" even when a narrow pane wraps the sentence.
            let note = (at[0]..at[1]).find(|&i| lines[i].trim().ends_with("repository root:"));
            Some(TrustPrompt {
                cli,
                folder: block_between(lines, at[0], note.unwrap_or(at[1]))?,
                repo_root: note.and_then(|note| block_between(lines, note, at[1])),
                trust_highlighted: lines[at[2]].trim() == "› 1. Trust and continue",
            })
        }
        // Claude Code 2.1.332:
        //   Accessing workspace:
        //   D:\...\worker-1
        //   Quick safety check: Is this a project you created or one you trust? ...
        //   ❯ No, exit
        //     Yes, I trust this folder
        //   Enter to confirm · Esc to cancel
        TrustCli::Claude => {
            let at = ordered(lines, &[
                &|l| l == "Accessing workspace:",
                &|l| l.contains("Is this a project you created or one you trust?"),
                &|l| l.ends_with("No, exit"),
                &|l| l.ends_with("Yes, I trust this folder"),
                &|l| l.starts_with("Enter to confirm"),
            ])?;
            Some(TrustPrompt {
                cli,
                folder: block_between(lines, at[0], at[1])?,
                repo_root: None,
                trust_highlighted: lines[at[3]].trim() == "❯ Yes, I trust this folder",
            })
        }
    }
}

type LineTest<'a> = &'a dyn Fn(&str) -> bool;

/// Row indexes of `steps` matched in order, each after the last, within [`DIALOG_SPAN`].
/// Tries each match of the first step, bottom-most first, so the live dialog wins over
/// an older copy higher up.
fn ordered(lines: &[String], steps: &[LineTest<'_>]) -> Option<Vec<usize>> {
    let (first, rest) = steps.split_first()?;
    let starts: Vec<usize> = (0..lines.len()).filter(|&i| first(lines[i].trim())).collect();
    starts.into_iter().rev().find_map(|start| {
        let end = (start + DIALOG_SPAN + 1).min(lines.len());
        let mut at = vec![start];
        for step in rest {
            let from = *at.last()? + 1;
            at.push((from..end).find(|&i| step(lines[i].trim()))?);
        }
        Some(at)
    })
}

/// Longest folder text read off a dialog. Windows' extended path limit is 32,767, but a
/// worktree path on a terminal is a few hundred characters at most.
const MAX_FOLDER_CHARS: usize = 512;

/// The non-blank text between two dialog rows, joined, so a long path the CLI wrapped
/// onto two rows reads back as one. Only the first paragraph counts, and it must look
/// like a single absolute path, because it is quoted to the Queen and operator.
fn block_between(lines: &[String], after: usize, before: usize) -> Option<String> {
    let block: String = lines[after + 1..before]
        .iter()
        .map(|line| line.trim())
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect();
    let absolute = block.starts_with('/') || block.get(1..3).is_some_and(|drive| drive == ":\\" || drive == ":/");
    let printable = !block.chars().any(|c| c.is_control() || c == '`');
    (absolute && printable && block.chars().count() <= MAX_FOLDER_CHARS).then_some(block)
}

/// Why a prompt is not answered automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The agent's folder is not a Hive-managed worktree, so its trust stays the
    /// operator's decision (a Queen on the project checkout, a shared-cell worker).
    NotManagedWorktree,
    /// The prompt names a folder other than the worker's worktree.
    ForeignPath,
    /// Codex would apply the trust to a repository root that is not the worktree's own.
    ForeignRepoRoot,
    /// Codex's repository root could not be read off the screen.
    UnreadRepoRoot,
    /// Claude Code's trust decision is always the operator's.
    OperatorDecides,
    /// The operator's own launch arguments set folder trust for this spawn.
    OperatorOverride,
    /// The highlighted option is not the trust choice.
    DefaultIsNotTrust,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotManagedWorktree => "not_a_managed_worktree",
            Self::ForeignPath => "foreign_path",
            Self::ForeignRepoRoot => "foreign_repo_root",
            Self::UnreadRepoRoot => "repo_root_unreadable",
            Self::OperatorDecides => "operator_decides",
            Self::OperatorOverride => "operator_trust_override",
            Self::DefaultIsNotTrust => "default_is_not_trust",
        }
    }
}

/// Whether `prompt` may be answered for a worker whose managed worktree is `cwd`.
pub fn check(prompt: &TrustPrompt, cwd: &str) -> Result<(), Refusal> {
    if prompt.cli == TrustCli::Claude {
        return Err(Refusal::OperatorDecides);
    }
    if normalize_path(&prompt.folder) != normalize_path(cwd) {
        return Err(Refusal::ForeignPath);
    }
    if prompt.cli == TrustCli::Codex {
        // Codex saves the decision at the repository root, so the root must be read and
        // must be this worktree's own main checkout. A linked worktree always shows it.
        let Some(root) = &prompt.repo_root else {
            return Err(Refusal::UnreadRepoRoot);
        };
        let own = git_repo_root(std::path::Path::new(cwd)).map(|own| normalize_path(&own.to_string_lossy()));
        if own.as_deref() != Some(normalize_path(root).as_str()) {
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
    /// Other input reached the terminal after the prompt was read, so the Enter was held.
    OperatorInput,
}

impl Escalation {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Reappeared => "reappeared",
            Self::Unchanged => "screen_unchanged",
            Self::WriteFailed => "write_failed",
            Self::OperatorInput => "operator_input",
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
    /// The prompt is gone, after an accept or after the operator answered it.
    Cleared(TrustPrompt),
    /// The accepted prompt did not go away: surface it.
    Escalated(TrustPrompt, Escalation),
}

#[derive(Debug, Clone)]
enum GuardState {
    Watching,
    Accepted { prompt: TrustPrompt, at_screen: Vec<String>, at: Instant },
    /// Handed to the operator. Never writes again; reports once when the prompt goes.
    Surfaced { prompt: TrustPrompt },
    Done,
}

/// Per-spawn trust-prompt state: a screen fed from the agent's output ring, plus the
/// once-only accept bookkeeping.
#[derive(Debug, Clone)]
pub struct TrustPromptGuard {
    cli: TrustCli,
    /// The worktree this guard may answer for, or why it may answer for none.
    authority: Result<String, Refusal>,
    started: Instant,
    screen: Screen,
    fed_to: Option<u64>,
    /// Turns of new output still owed after the grid was thrown away (resize or
    /// eviction) before it counts as a whole new frame. While non-zero, a prompt is
    /// never reported cleared.
    rebuilding: u8,
    state: GuardState,
}

impl TrustPromptGuard {
    /// `authority` is the seat's own managed worktree, already verified by the caller
    /// against the session (`cli::folder_trust::managed_worktree_project`), or the reason
    /// it has none; then every prompt is surfaced with that reason.
    pub fn new(cli: TrustCli, authority: Result<String, Refusal>, cols: u16, rows: u16, started: Instant) -> Self {
        Self {
            cli,
            authority,
            started,
            screen: Screen::new(cols, rows),
            fed_to: None,
            rebuilding: 0,
            state: GuardState::Watching,
        }
    }

    /// Whether the guard still needs output. A finished guard never acts again, which is
    /// what makes the accept once-per-spawn.
    pub fn is_active(&self, now: Instant) -> bool {
        match self.state {
            GuardState::Done => false,
            GuardState::Watching => now.duration_since(self.started) < WATCH_WINDOW,
            GuardState::Accepted { .. } | GuardState::Surfaced { .. } => true,
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
            self.finish();
            return None;
        }
        let (reset, new) = self.feed(snapshot, cols, rows);
        let lines = self.screen.lines();
        let shown = detect(self.cli, &lines);
        // A thrown-away grid cannot prove that a prompt went away. It is whole again when
        // the prompt shows, when new output clears the full screen, or after
        // REBUILD_TURNS further turns that each drew something.
        if reset {
            self.rebuilding = REBUILD_TURNS + 1;
        }
        if shown.is_some() || clears_screen(&new) {
            self.rebuilding = 0;
        } else if self.rebuilding > 0 && !new.is_empty() {
            self.rebuilding -= 1;
        }
        if shown.is_none() && self.rebuilding > 0 {
            return None;
        }
        let (next, action) = match std::mem::replace(&mut self.state, GuardState::Done) {
            GuardState::Watching => match shown {
                None => (GuardState::Watching, None),
                Some(prompt) => self.first_sight(prompt, lines, now),
            },
            GuardState::Accepted { prompt, at_screen, at } => {
                let waited = now.duration_since(at);
                match shown {
                    None => (GuardState::Done, Some(GuardAction::Cleared(prompt))),
                    Some(_) if lines == at_screen && waited < UNCHANGED_LIMIT => {
                        (GuardState::Accepted { prompt, at_screen, at }, None)
                    }
                    Some(_) if lines != at_screen && waited < REAPPEAR_GRACE => {
                        (GuardState::Accepted { prompt, at_screen, at }, None)
                    }
                    Some(_) => {
                        let why = if lines == at_screen { Escalation::Unchanged } else { Escalation::Reappeared };
                        (GuardState::Surfaced { prompt: prompt.clone() }, Some(GuardAction::Escalated(prompt, why)))
                    }
                }
            }
            GuardState::Surfaced { prompt } => match shown {
                None => (GuardState::Done, Some(GuardAction::Cleared(prompt))),
                Some(_) => (GuardState::Surfaced { prompt }, None),
            },
            GuardState::Done => (GuardState::Done, None),
        };
        self.state = next;
        if matches!(self.state, GuardState::Done) {
            self.finish();
        }
        action
    }

    fn first_sight(&self, prompt: TrustPrompt, lines: Vec<String>, now: Instant) -> (GuardState, Option<GuardAction>) {
        let verdict = match &self.authority {
            Ok(cwd) => check(&prompt, cwd),
            Err(refusal) => Err(*refusal),
        };
        match verdict {
            Ok(()) => (
                GuardState::Accepted { prompt: prompt.clone(), at_screen: lines, at: now },
                Some(GuardAction::Accept(prompt)),
            ),
            Err(refusal) => (GuardState::Surfaced { prompt: prompt.clone() }, Some(GuardAction::Refused(prompt, refusal))),
        }
    }

    /// Record the outcome of writing the accept keys: `None` when it was written, else
    /// why it was not. Anything but a write is escalated and never retried.
    pub fn accept_written(&mut self, failure: Option<Escalation>) -> Option<GuardAction> {
        let failure = failure?;
        let GuardState::Accepted { prompt, .. } = &self.state else {
            return None;
        };
        let prompt = prompt.clone();
        self.state = GuardState::Surfaced { prompt: prompt.clone() };
        Some(GuardAction::Escalated(prompt, failure))
    }

    /// Stop for good and release the screen; only the spawn marker stays behind.
    pub fn finish(&mut self) {
        self.state = GuardState::Done;
        self.screen = Screen::new(1, 1);
    }

    /// Returns whether the grid was thrown away and started again, and the new bytes drawn.
    fn feed(&mut self, snapshot: &OutputSnapshot, cols: u16, rows: u16) -> (bool, Vec<u8>) {
        // A resize makes ConPTY repaint the whole viewport, so a fresh grid at the new
        // size is filled again by the following frames.
        let mut reset = self.screen.size() != (cols, rows);
        if reset {
            self.screen = Screen::new(cols, rows);
        }
        let from = match self.fed_to {
            // Output we never saw was evicted: start again from what is retained.
            Some(fed) if fed < snapshot.offset_start => {
                self.screen = Screen::new(cols, rows);
                reset = true;
                snapshot.offset_start
            }
            Some(fed) => fed,
            None => snapshot.offset_start,
        };
        let skip = from.saturating_sub(snapshot.offset_start) as usize;
        let new = snapshot.data.get(skip..).unwrap_or_default();
        self.screen.feed(new);
        self.fed_to = Some(snapshot.offset_end.max(from));
        (reset, new.to_vec())
    }
}

#[cfg(test)]
#[path = "trust_prompt_tests.rs"]
pub(crate) mod tests;
