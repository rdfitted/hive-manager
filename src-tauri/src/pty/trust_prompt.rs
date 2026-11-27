//! Detect a CLI's folder-trust prompt on a worker's PTY screen and surface it (#466).
//!
//! Prevention (`cli::folder_trust`) pre-trusts a seat's own worktree for codex and agy,
//! so a prompt here means prevention did not apply (another folder, an operator override,
//! a CLI version that ignores it) or the CLI has no override at all (Claude Code).
//! Hive Manager never answers it: a screen match cannot prove which dialog has the
//! terminal's input, so the prompt is surfaced to the Queen and the operator and the
//! agent shows `WaitingForInput("trust_prompt")` until it goes (Ryan, 2026-11-27).
//!
//! The prompt texts below come from real captures, kept as fixtures in
//! `trust_prompt_fixtures/`: agy 1.5.18, codex-cli 0.171.0 and Claude Code 2.1.332, each
//! spawned with the adapter's own flags into a fresh, untrusted worktree.

use std::time::{Duration, Instant};

use super::output_ring::OutputSnapshot;
use super::screen::Screen;

/// Non-empty rows a frame drawn after a grid reset must have before an absent prompt
/// counts as gone, unless the frame starts with a full-screen clear. A partial repaint
/// (a cursor move, one cell) never qualifies.
const MIN_FRAME_ROWS: usize = 3;

/// Bytes of each read kept so a clear sequence split across reads is still seen: one
/// less than the longest sequence `clears_screen` matches (`ESC[?1049h`).
const CLEAR_TAIL: usize = 7;

/// Whether `bytes` redraw the whole screen from blank: erase display, full reset, or a
/// switch into or out of the alternate screen (agy leaves its trust dialog that way).
fn clears_screen(bytes: &[u8]) -> bool {
    clears_screen_after(bytes, 0)
}

/// Whether `bytes` hold a full-screen clear that ends after the first `carried` bytes,
/// which were already seen in an earlier read.
fn clears_screen_after(bytes: &[u8], carried: usize) -> bool {
    [
        b"\x1b[2J".as_slice(), b"\x1bc", b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?1047h", b"\x1b[?1047l",
        b"\x1b[?47h", b"\x1b[?47l",
    ]
        .iter()
        .any(|needle| {
            bytes.windows(needle.len()).enumerate().any(|(at, window)| window == *needle && at + needle.len() > carried)
        })
}

/// The status reason set while a trust prompt is on screen.
pub const TRUST_PROMPT_STATUS: &str = "trust_prompt";


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
                &|l| option(l) == "Yes, I trust this folder",
                &|l| option(l) == "No, exit",
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
                // Its opening words only: a narrow pane wraps the rest of the question.
                &|l| l.starts_with("Trust this folder?"),
                &|l| option(l) == "1. Trust and continue",
                &|l| option(l).starts_with("2. "),
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
                &|l| l.contains("Is this a project you created"),
                &|l| option(l) == "No, exit",
                &|l| option(l) == "Yes, I trust this folder",
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

/// An option row without its selection marker, so the dialog still matches whichever
/// option the operator has moved the selection to.
fn option(line: &str) -> &str {
    line.trim_start_matches(['>', '\u{203a}', '\u{276f}']).trim_start()
}

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
    let absolute = block.starts_with('/')
        || block.starts_with("\\\\")
        || block.get(1..3).is_some_and(|drive| drive == ":\\" || drive == ":/");
    let printable = !block.chars().any(|c| c.is_control() || c == '`');
    (absolute && printable && block.chars().count() <= MAX_FOLDER_CHARS).then_some(block)
}

/// What one observation reports. Neither sends anything to the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardAction {
    /// A trust prompt is up: surface it to the Queen and the operator.
    Detected(TrustPrompt),
    /// The prompt is gone; the operator (or the CLI) dealt with it.
    Cleared(TrustPrompt),
}

#[derive(Debug, Clone)]
enum GuardState {
    Watching,
    /// Surfaced once; watched passively until the prompt goes.
    Surfaced { prompt: TrustPrompt },
    Done,
}

/// Per-spawn trust-prompt state: a screen fed from the agent's output ring, and whether
/// the prompt has been surfaced. It never writes to the PTY.
#[derive(Debug, Clone)]
pub struct TrustPromptGuard {
    cli: TrustCli,
    started: Instant,
    screen: Screen,
    fed_to: Option<u64>,
    /// Set when the grid was thrown away (resize or eviction). While set, a prompt is
    /// never reported cleared.
    rebuilding: bool,
    /// The end of the last read, so a clear split across reads still counts.
    clear_tail: Vec<u8>,
    state: GuardState,
}

impl TrustPromptGuard {
    pub fn new(cli: TrustCli, cols: u16, rows: u16, started: Instant) -> Self {
        Self {
            cli,
            started,
            screen: Screen::new(cols, rows),
            fed_to: None,
            rebuilding: false,
            clear_tail: Vec::new(),
            state: GuardState::Watching,
        }
    }

    /// Whether the guard still needs output. A guard that saw no prompt within the watch
    /// window, or saw its prompt go, is finished.
    pub fn is_active(&self, now: Instant) -> bool {
        match self.state {
            GuardState::Done => false,
            GuardState::Watching => now.duration_since(self.started) < WATCH_WINDOW,
            GuardState::Surfaced { .. } => true,
        }
    }

    /// Whether the guard has surfaced a prompt that is still up.
    #[cfg(test)]
    pub fn is_surfaced(&self) -> bool {
        matches!(self.state, GuardState::Surfaced { .. })
    }

    /// The absolute offset the guard has rendered up to; pass it to the output source.
    pub fn fed_to(&self) -> u64 {
        self.fed_to.unwrap_or(0)
    }

    /// Render the output in `snapshot` that is new since the last call, at the PTY's
    /// current size, and report a prompt appearing or going.
    pub fn observe(&mut self, snapshot: &OutputSnapshot, cols: u16, rows: u16, now: Instant) -> Option<GuardAction> {
        if !self.is_active(now) {
            self.finish();
            return None;
        }
        let (reset, new) = self.feed(snapshot, cols, rows);
        let lines = self.screen.lines();
        let shown = detect(self.cli, &lines);
        if self.still_rebuilding(reset, &new, shown.is_some(), &lines) {
            return None;
        }
        match (std::mem::replace(&mut self.state, GuardState::Done), shown) {
            (GuardState::Watching, None) => {
                self.state = GuardState::Watching;
                None
            }
            (GuardState::Watching, Some(prompt)) => {
                self.state = GuardState::Surfaced { prompt: prompt.clone() };
                Some(GuardAction::Detected(prompt))
            }
            (GuardState::Surfaced { prompt }, None) => {
                self.finish();
                Some(GuardAction::Cleared(prompt))
            }
            (GuardState::Surfaced { prompt }, Some(_)) => {
                self.state = GuardState::Surfaced { prompt };
                None
            }
            (GuardState::Done, _) => None,
        }
    }

    /// A thrown-away grid cannot prove that a prompt went away, and nothing hangs on a
    /// notice that stays up, so the guard waits for a real frame: the prompt showing
    /// again, or new output that draws a whole screen without it (a full-screen clear
    /// followed by content, or at least MIN_FRAME_ROWS non-empty rows). Time alone, an
    /// empty grid or escape-only output never ends the wait.
    fn still_rebuilding(&mut self, reset: bool, new: &[u8], shown: bool, lines: &[String]) -> bool {
        if reset {
            self.rebuilding = true;
            self.clear_tail.clear();
        }
        let mut seen = std::mem::take(&mut self.clear_tail);
        let carried = seen.len();
        seen.extend_from_slice(new);
        self.clear_tail = seen[seen.len().saturating_sub(CLEAR_TAIL)..].to_vec();
        if !self.rebuilding {
            return false;
        }
        // A clear counts with the content drawn in the same read, even when its escape
        // sequence started at the end of the previous read. A clear whose content comes
        // in later reads needs MIN_FRAME_ROWS rows like any frame: a partial repaint of a
        // still-open dialog must not end the notice.
        let drawn = lines.iter().filter(|line| !line.trim().is_empty()).count();
        let whole_frame = !new.is_empty() && ((clears_screen_after(&seen, carried) && drawn > 0) || drawn >= MIN_FRAME_ROWS);
        if shown || whole_frame {
            self.rebuilding = false;
            return false;
        }
        true
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
        // Compare clamped sizes, or a PTY beyond the grid's limits would reset every turn.
        let mut reset = self.screen.size() != Screen::clamped(cols, rows);
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
