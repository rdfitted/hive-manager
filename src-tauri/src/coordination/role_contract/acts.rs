//! Deterministic speech-act mapper for role-contract checks.
//!
//! ATC (arXiv:2611.04321, Section 3.2) maps a proposed message to interaction
//! functions with a fixed classifier and checks them against the role's
//! required and prohibited behaviors. This module is that mapper, built from
//! fixed patterns instead of a learned classifier, so a check is reproducible
//! and gradeable without a model. It detects first-person claims or
//! intentions to act ("I pushed", "I'll merge"), shell commands that perform
//! the act, and evidence markers. Negated, interrogative and third-party
//! sentences do not count as acts.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// An interaction function a message can perform. Contracts reference these
/// by their snake_case names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    /// Claims to have edited, patched or committed code.
    ClaimsCodeEdit,
    /// Claims to have pushed (or to be about to push) a branch.
    ClaimsPush,
    /// Claims to have merged (or to be about to merge) a PR or branch.
    ClaimsMerge,
    /// Claims to have deployed, released, tagged or published.
    ClaimsDeploy,
    /// Claims to have posted outside the hive: PRs, issue or PR comments.
    ClaimsExternalPost,
    /// Spawns or intends to spawn a hive-managed principal. QA-worker spawns
    /// are excluded: the Evaluator owns them through its own route.
    Spawns,
    /// Decomposes work or assigns it to other agents.
    PlansWork,
    /// Issues a verification verdict or approval.
    IssuesVerdict,
    /// Overrides, ignores or force-passes a verification verdict.
    OverridesVerdict,
    /// Takes over another principal's lane.
    TakesOverLane,
    /// Declares its own task, lane or milestone done.
    DeclaresDone,
}

/// One detected act and the bounded excerpt that triggered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActHit {
    pub act: Act,
    pub snippet: String,
}

/// The mapper output: detected acts plus whether evidence markers are present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MessageActs {
    pub hits: Vec<ActHit>,
    pub has_evidence: bool,
}

impl MessageActs {
    pub fn contains(&self, act: Act) -> bool {
        self.hits.iter().any(|hit| hit.act == act)
    }
}

const MAX_SNIPPET_CHARS: usize = 160;

// First-person subject, then up to three filler words, then the verb. The
// filler group is inspected for negation, because the regex crate has no
// lookbehind.
const SUBJECT: &str = r"\b(?:i|i've|ive|i'll|ill|i'm|im|i'd|we|we've|we'll|we're)\b";
const FILLER: &str = r"(?P<mid>(?:\s+[a-z'’-]+){0,3}?)";

struct Detector {
    act: Act,
    /// First-person forms: the `mid` capture is checked for negation.
    claims: Vec<Regex>,
    /// Command or impersonal forms: the whole sentence is checked for negation.
    commands: Vec<Regex>,
}

fn claim(verbs: &str, object: &str) -> Regex {
    Regex::new(&format!(r"(?i){SUBJECT}{FILLER}\s+(?:{verbs}){object}")).expect("valid claim pattern")
}

fn pattern(source: &str) -> Regex {
    Regex::new(&format!("(?i){source}")).expect("valid act pattern")
}

// A code-like object: a source path, or a noun phrase that names code.
const CODE_OBJECT: &str = r"\b[^.!?\n]{0,60}?(?:\b[\w./\\-]+\.(?:rs|ts|tsx|js|svelte|py|toml|json|ya?ml|css|html|sh|ps1)\b|\b(?:the|this|that|a|your|their|w\d+'s|worker'?s?)\s+(?:[\w-]+\s+){0,2}(?:code|bug|handler|implementation|function|module|test|tests|loop|parser|fix|file|files|logic|lane|branch|diff)\b)";

fn detectors() -> &'static [Detector] {
    static DETECTORS: OnceLock<Vec<Detector>> = OnceLock::new();
    DETECTORS.get_or_init(|| {
        vec![
            Detector {
                act: Act::ClaimsCodeEdit,
                claims: vec![
                    claim(
                        r"edit(?:ed)?|modif(?:y|ied)|patch(?:ed)?|chang(?:e|ed)|fix(?:ed)?|rewr(?:ite|ote|itten)|refactor(?:ed)?|implement(?:ed)?|updat(?:e|ed)|add(?:ed)?|remov(?:e|ed)|apply|applied",
                        CODE_OBJECT,
                    ),
                    // Pronoun objects: "I patched it myself".
                    claim(
                        r"edit(?:ed)?|modif(?:y|ied)|patch(?:ed)?|fix(?:ed)?|rewr(?:ite|ote|itten)|refactor(?:ed)?",
                        r"\s+(?:it|this|that|them)\b",
                    ),
                    claim(r"commit(?:ted)?|amend(?:ed)?", r"\b"),
                    claim(r"went\s+ahead\s+and\s+(?:edit|fix|patch|rewr|chang|implement)", r""),
                ],
                commands: vec![
                    pattern(r"\bgit\s+commit\b"),
                    // Subject-less continuation: "I reviewed it and committed".
                    pattern(r"\band\s+(?:then\s+)?(?:committed|patched|rewrote)\b"),
                ],
            },
            Detector {
                act: Act::ClaimsPush,
                claims: vec![claim(r"push(?:ed|ing)?|force[- ]push(?:ed|ing)?", r"\b")],
                commands: vec![
                    pattern(r"\bgit\s+push\b"),
                    pattern(r"^\s*(?:force[- ])?push(?:ed|ing)\b"),
                    pattern(r"\band\s+(?:then\s+)?(?:force[- ])?pushed\b"),
                ],
            },
            Detector {
                act: Act::ClaimsMerge,
                claims: vec![claim(
                    r"merg(?:e|ed|ing)|squash[- ]merg(?:e|ed|ing)",
                    r"\b[^.!?\n]{0,40}?\b(?:pr|pull request|branch|main|staging|it|this|that|#\d+|changes|variant)\b",
                )],
                commands: vec![
                    pattern(r"\bgh\s+pr\s+merge\b"),
                    pattern(r"\bgit\s+merge\b[^.!?\n]*\b(?:main|staging|master)\b"),
                    pattern(r"^\s*merged\s+(?:pr|#\d+|the\s+pr|it)\b"),
                    pattern(r"\band\s+(?:then\s+)?(?:squash[- ])?merged\s+(?:pr\b|#\d+|it\b|the\s+pr|this\b|that\b|into\b)"),
                ],
            },
            Detector {
                act: Act::ClaimsDeploy,
                claims: vec![
                    claim(r"deploy(?:ed|ing)?|releas(?:e|ed|ing)|publish(?:ed|ing)?\s+(?:the\s+)?(?:release|package|crate|build)|tagg?(?:ed)?\s+v?\d", r"\b"),
                ],
                commands: vec![
                    pattern(r"\bgh\s+release\s+create\b"),
                    pattern(r"\b(?:npm|cargo)\s+publish\b"),
                    pattern(r"\bgit\s+tag\s+v?\d"),
                    pattern(r"^\s*(?:deployed|released)\b"),
                ],
            },
            Detector {
                act: Act::ClaimsExternalPost,
                claims: vec![
                    claim(r"open(?:ed)?|creat(?:e|ed)|fil(?:e|ed)|rais(?:e|ed)", r"\s+(?:a\s+|the\s+|an\s+)?(?:new\s+)?(?:pr|pull request|github issue|issue)\b"),
                    claim(r"comment(?:ed)?|post(?:ed)?|repl(?:y|ied)", r"\b[^.!?\n]{0,30}?\b(?:on|to)\s+(?:the\s+)?(?:pr|pull request|issue|github|#\d+)"),
                ],
                commands: vec![
                    pattern(r"\bgh\s+pr\s+(?:create|comment|review)\b"),
                    pattern(r"\bgh\s+issue\s+(?:create|comment)\b"),
                    pattern(r"^\s*opened\s+(?:pr|a\s+pr|the\s+pr|pull request)\b"),
                    pattern(r"\band\s+(?:then\s+)?opened\s+(?:a\s+|the\s+)?(?:new\s+)?(?:pr|pull request)\b"),
                ],
            },
            Detector {
                act: Act::Spawns,
                claims: vec![claim(
                    r"spawn(?:ed|ing)?|launch(?:ed|ing)?|start(?:ed|ing)?",
                    r"\s+(?:a\s+|an\s+|another\s+|one\s+more\s+|\d+\s+)?(?:new\s+)?(?:hive\s+)?(?:worker|workers|fixer|fixers|principal|planner)\b",
                )],
                commands: vec![
                    pattern(r"\bspawning\s+(?:a\s+|an\s+|another\s+)?(?:new\s+)?(?:hive\s+)?(?:worker|fixer|principal)\b"),
                    pattern(r"/api/sessions/[^\s/]+/(?:workers|planners)\b"),
                ],
            },
            Detector {
                act: Act::PlansWork,
                claims: vec![
                    claim(r"assign(?:ed|ing)?|delegat(?:e|ed|ing)|reassign(?:ed|ing)?", r"\b[^.!?\n]{0,40}?\bto\s+(?:w\d+|worker|the\s+worker|a\s+fixer|another)"),
                    claim(r"break(?:ing)?|split(?:ting)?|decompos(?:e|ing)", r"\b[^.!?\n]{0,30}?\binto\s+(?:\w+\s+)?(?:tasks|lanes|subtasks)"),
                ],
                commands: vec![
                    pattern(r"\bhere(?:'s|’s| is)\s+the\s+(?:new\s+)?(?:implementation|remediation|fix|task)\s+plan\b"),
                    pattern(r"\b(?:w\d+|worker\s*\d+)\s+should\s+(?:implement|fix|build|write|add|change|refactor)\b"),
                ],
            },
            Detector {
                act: Act::IssuesVerdict,
                claims: vec![
                    claim(r"approv(?:e|ed|ing)|sign(?:ed|ing)?\s+off|certif(?:y|ied)", r"\b[^.!?\n]{0,30}?\b(?:pr|change|changes|milestone|this|it|lane|work)\b"),
                    claim(r"mark(?:ed|ing)?|declar(?:e|ed|ing)", r"\b[^.!?\n]{0,30}?\bas\s+(?:passed|passing|approved|verified)\b"),
                ],
                commands: vec![
                    pattern(r"\b(?:qa\s+)?verdict\s*[:=]\s*\**\s*(?:pass|fail|passed|failed)\b"),
                    pattern(r"\blgtm\b"),
                ],
            },
            Detector {
                act: Act::OverridesVerdict,
                claims: vec![claim(
                    r"overrid(?:e|ing)|overr(?:ode|uled?)|ignor(?:e|ing)|disregard(?:ing)?|overturn(?:ing|ed)?",
                    r"\b[^.!?\n]{0,30}?\b(?:verdict|evaluator|qa|fail|failure|findings)\b",
                )],
                commands: vec![
                    pattern(r"\bforce[- ]?(?:pass|fail)\b"),
                    pattern(r"\bignore\s+the\s+(?:qa|evaluator)\b"),
                    pattern(r"\btreat\s+the\s+fail\s+as\s+a\s+pass\b"),
                ],
            },
            Detector {
                act: Act::TakesOverLane,
                claims: vec![
                    claim(r"implement|do|write|fix|build|finish|handle|code", r"\s+(?:it|this|that|the\s+\w+\s+lane|their\s+lane|w\d+'s\s+\w+|the\s+\w+\s+myself)[^.!?\n]{0,20}?\bmyself\b"),
                    claim(r"tak(?:e|ing)\s+over", r"\b[^.!?\n]{0,20}?\b(?:w\d+|worker|lane|their)\b"),
                ],
                commands: vec![],
            },
            Detector {
                act: Act::DeclaresDone,
                claims: vec![claim(r"(?:am|have|'m|'ve)?\s*(?:done|finished|completed)", r"(?:\s+(?:with\s+)?(?:it|this|my\s+\w+|the\s+\w+))?\s*(?:[.!,;:]|$)")],
                commands: vec![
                    pattern(r"\b(?:my\s+)?(?:task|lane|work|implementation|milestone|assignment|ticket)\s+(?:is\s+)?(?:now\s+)?(?:complete|completed|done|finished)\b"),
                    pattern(r"^\s*(?:status:\s*)?(?:completed|done|task\s+complete)\b"),
                    pattern(r"\ball\s+done\b"),
                    pattern(r"\bmarking\s+(?:myself|my\s+task|this|it|the\s+task)\s+(?:as\s+)?(?:complete|completed|done)\b"),
                ],
            },
        ]
    })
}

fn negation() -> &'static Regex {
    static NEGATION: OnceLock<Regex> = OnceLock::new();
    NEGATION.get_or_init(|| pattern(r"\b(?:not|never|no|don't|dont|do not|didn't|didnt|won't|wont|can't|cant|cannot|haven't|havent|hasn't|shouldn't|mustn't|must not|without|nobody|nothing)\b|n't\b|n’t\b"))
}

fn evidence_patterns() -> &'static [Regex] {
    static EVIDENCE: OnceLock<Vec<Regex>> = OnceLock::new();
    EVIDENCE.get_or_init(|| {
        vec![
            pattern(r"\btest result:\s*ok\b"),
            pattern(r"\b\d+\s+(?:tests?\s+)?pass(?:ed|ing)?\b"),
            pattern(r"\b0\s+failed\b"),
            pattern(r"\bexit(?:ed)?\s+(?:code\s+|status\s+)?0\b"),
            pattern(r"\b(?:cargo|npm|pnpm|yarn|npx|pytest|vitest|go|python)\s+(?:run\s+)?(?:test|check|clippy|vitest|-m\s+unittest)\b"),
            pattern(r"\b(?:pr|issue)\s*#\d+\b"),
            pattern(r"\bevidence\s*:"),
        ]
    })
}

fn commit_sha() -> &'static Regex {
    static SHA: OnceLock<Regex> = OnceLock::new();
    SHA.get_or_init(|| pattern(r"\b[0-9a-f]{7,40}\b"))
}

fn is_sha(candidate: &str) -> bool {
    candidate.chars().any(|c| c.is_ascii_digit()) && candidate.chars().any(|c| c.is_ascii_alphabetic())
}

fn file_reference() -> &'static Regex {
    static FILE: OnceLock<Regex> = OnceLock::new();
    FILE.get_or_init(|| {
        pattern(r"(?P<path>[\w./\\:-]*[\w-]\.(?:rs|ts|tsx|js|svelte|py|md|json|toml|ya?ml|png))\b(?::(?P<line>\d+)(?:-\d+)?)?")
    })
}

/// Bounds that keep a check cheap: at most this many distinct SHA lookups
/// (one `git cat-file` per root each) and file probes per message.
const MAX_SHA_LOOKUPS: usize = 8;
const MAX_FILE_PROBES: usize = 16;
/// Files larger than this count as existing without their lines being counted.
const MAX_LINE_COUNT_BYTES: u64 = 4 * 1024 * 1024;

/// Evidence: test output, a test command, a PR or issue number, an explicit
/// "evidence:" label, or a citation that resolves. A commit SHA counts only
/// when it names a commit in one of `roots`; a file reference only when the
/// file exists under one of them (and a cited line is within the file). An
/// unresolved citation is not evidence, and with no roots nothing resolves.
pub fn has_evidence(text: &str, roots: &[PathBuf]) -> bool {
    if evidence_patterns().iter().any(|regex| regex.is_match(text)) {
        return true;
    }
    if roots.is_empty() {
        return false;
    }
    for captures in file_reference().captures_iter(text).take(MAX_FILE_PROBES) {
        let path = captures.name("path").map_or("", |found| found.as_str());
        let line = captures.name("line").and_then(|found| found.as_str().parse::<u64>().ok());
        if roots.iter().any(|root| citation_resolves(root, path, line)) {
            return true;
        }
    }
    let mut seen: Vec<String> = Vec::new();
    for found in commit_sha().find_iter(text) {
        let sha = found.as_str().to_ascii_lowercase();
        if !is_sha(&sha) || seen.contains(&sha) {
            continue;
        }
        if seen.len() == MAX_SHA_LOOKUPS {
            break;
        }
        if roots.iter().any(|root| commit_resolves(root, &sha)) {
            return true;
        }
        seen.push(sha);
    }
    false
}

/// A path is checked under `root` directly. A bare file name ("queue.rs:88",
/// the way agents usually cite) is looked up among the checkout's tracked and
/// untracked-but-not-ignored files, and resolves if any match does.
fn citation_resolves(root: &Path, cited: &str, line: Option<u64>) -> bool {
    if cited.contains(['/', '\\']) || Path::new(cited).is_absolute() {
        return file_resolves(root, cited, line);
    }
    bare_name_matches(root, cited)
        .iter()
        .any(|relative| file_resolves(root, relative, line))
}

fn bare_name_matches(root: &Path, name: &str) -> Vec<String> {
    if !root.is_dir() || name.starts_with(['-', ':']) {
        return Vec::new();
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--"])
        .arg(name)
        .arg(format!("*/{name}"))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    crate::process::hide_std_console_window(&mut command);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter(|path| path.rsplit('/').next().is_some_and(|file| file.eq_ignore_ascii_case(name)))
        .take(MAX_FILE_PROBES)
        .map(str::to_string)
        .collect()
}

/// A relative path must stay inside `root`; an absolute one must lie under it.
fn file_resolves(root: &Path, cited: &str, line: Option<u64>) -> bool {
    let cited_path = Path::new(cited);
    let candidate = if cited_path.is_absolute() {
        cited_path.to_path_buf()
    } else {
        if cited_path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
        {
            return false;
        }
        root.join(cited_path)
    };
    let (Ok(root), Ok(candidate)) = (root.canonicalize(), candidate.canonicalize()) else {
        return false;
    };
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return false;
    }
    let Some(line) = line else {
        return true;
    };
    if line == 0 {
        return false;
    }
    match std::fs::metadata(&candidate) {
        Ok(meta) if meta.len() > MAX_LINE_COUNT_BYTES => true,
        Ok(_) => std::fs::read(&candidate)
            .map(|bytes| {
                let breaks = bytes.iter().filter(|&&byte| byte == b'\n').count() as u64;
                let lines = breaks + u64::from(bytes.last().is_some_and(|&byte| byte != b'\n'));
                line <= lines
            })
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// `git cat-file -e <sha>^{commit}` in `root`: windowless, no output kept.
fn commit_resolves(root: &Path, sha: &str) -> bool {
    if !root.is_dir() {
        return false;
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::hide_std_console_window(&mut command);
    command.status().is_ok_and(|status| status.success())
}

/// Split into sentences, keeping the terminator so questions are recognisable.
/// Inline code spans are kept, since a command inside backticks is still a command.
fn sentences(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        let boundary = match byte {
            b'\n' | b'!' | b'?' | b';' => true,
            // A period ends a sentence only before whitespace or the end, so
            // paths like `inject.rs:812` and versions like 0.67.1 stay intact.
            b'.' => bytes.get(index + 1).is_none_or(|next| next.is_ascii_whitespace()),
            _ => false,
        };
        if boundary {
            let sentence = &text[start..=index];
            out.push((sentence, byte == b'?'));
            start = index + 1;
        }
    }
    if start < text.len() {
        out.push((&text[start..], false));
    }
    out.into_iter()
        .map(|(sentence, question)| (sentence.trim(), question))
        .filter(|(sentence, _)| !sentence.is_empty())
        .collect()
}

fn snippet(sentence: &str) -> String {
    sentence.chars().take(MAX_SNIPPET_CHARS).collect()
}

/// Map a message to its acts. `declares_done` is forced when the transport
/// itself declares completion (a `completed` heartbeat). Cited commits and
/// files must resolve in one of `evidence_roots` to count as evidence.
pub fn map_message(text: &str, transport_declares_done: bool, evidence_roots: &[PathBuf]) -> MessageActs {
    let normalized = text.replace(['’', '‘'], "'");
    let mut hits: Vec<ActHit> = Vec::new();
    for (sentence, question) in sentences(&normalized) {
        // A question asks for authority rather than exercising it.
        if question {
            continue;
        }
        for detector in detectors() {
            if hits.iter().any(|hit| hit.act == detector.act) {
                continue;
            }
            let by_claim = detector.claims.iter().any(|regex| {
                regex.captures_iter(sentence).any(|captures| {
                    let mid = captures.name("mid").map_or("", |found| found.as_str());
                    !negation().is_match(mid)
                })
            });
            // A command counts unless a negation precedes it in the same
            // sentence ("do not run git push"). A later clause such as
            // "so the Queen doesn't have to" does not cancel it.
            let by_command = detector.commands.iter().any(|regex| {
                regex
                    .find_iter(sentence)
                    .any(|found| !negation().is_match(&sentence[..found.start()]))
            });
            if by_claim || by_command {
                hits.push(ActHit { act: detector.act, snippet: snippet(sentence) });
            }
        }
    }
    if transport_declares_done && !hits.iter().any(|hit| hit.act == Act::DeclaresDone) {
        hits.push(ActHit {
            act: Act::DeclaresDone,
            snippet: "status=completed".to_string(),
        });
    }
    hits.sort_by_key(|hit| hit.act);
    MessageActs {
        has_evidence: has_evidence(&normalized, evidence_roots),
        hits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acts(text: &str) -> Vec<Act> {
        map_message(text, false, &[]).hits.into_iter().map(|hit| hit.act).collect()
    }

    #[test]
    fn first_person_claims_are_detected() {
        assert_eq!(acts("I pushed the branch."), vec![Act::ClaimsPush]);
        assert_eq!(acts("I'll merge the PR now."), vec![Act::ClaimsMerge]);
        assert_eq!(acts("I patched src/foo.rs myself."), vec![Act::ClaimsCodeEdit]);
    }

    #[test]
    fn negated_and_questioned_claims_are_not_acts() {
        assert!(acts("I did not push anything.").is_empty());
        assert!(acts("I won't merge; the operator merges.").is_empty());
        assert!(acts("Should I push my branch?").is_empty());
        assert!(acts("Do not run git push.").is_empty());
    }

    #[test]
    fn paths_and_versions_do_not_split_sentences() {
        assert_eq!(sentences("See inject.rs:812 and 0.67.1 now. Next").len(), 2);
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    }

    #[test]
    fn evidence_requires_a_real_marker() {
        assert!(has_evidence("cargo test --lib: 42 passed", &[]));
        assert!(has_evidence("evidence: see the QA log", &[]));
        assert!(!has_evidence("Done, everything works.", &[repo_root()]));
        assert!(!has_evidence("deadbeef is not a sha without digits", &[repo_root()]));
    }

    #[test]
    fn unresolved_citations_are_not_evidence() {
        let roots = [repo_root()];
        // Marker-shaped, but naming no commit and no file in the repo.
        assert!(!has_evidence("commit 3fa9c1e", &roots));
        assert!(!has_evidence("Fixed in src/no_such_module_anywhere.rs:12.", &roots));
        assert!(!has_evidence("See src-tauri/src/lib.rs:999999999.", &roots));
        assert!(!has_evidence("See src-tauri/src/lib.rs:0.", &roots));
        assert!(!has_evidence("See ../outside/README.md for it.", &roots));
        assert!(!has_evidence("Broken at no_such_file_anywhere.rs:3.", &roots));
        assert!(!has_evidence("See eval.rs:999999.", &roots));
        // Nothing resolves without a checkout to resolve in.
        assert!(!has_evidence("See src-tauri/src/lib.rs:1.", &[]));
    }

    #[test]
    fn resolving_citations_are_evidence() {
        let root = repo_root();
        let roots = std::slice::from_ref(&root);
        assert!(has_evidence("Done; see src-tauri/src/lib.rs:1.", roots));
        assert!(has_evidence("Updated tools/role-contracts/README.md", roots));
        // A bare file name, the way agents usually cite one.
        assert!(has_evidence("The new test at acts.rs:1 never runs.", roots));
        let head = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--short=10", "HEAD"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default();
        // A tarball checkout has no history; the file cases above still run.
        if is_sha(&head) {
            assert!(has_evidence(&format!("Landed as {head}."), roots));
        }
    }

    #[test]
    fn completed_transport_forces_declares_done() {
        let mapped = map_message("", true, &[]);
        assert!(mapped.contains(Act::DeclaresDone));
        assert!(!mapped.has_evidence);
    }
}
