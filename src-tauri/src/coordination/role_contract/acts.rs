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
            pattern(r"\b[\w./\\-]+\.(?:rs|ts|tsx|js|svelte|py|md|json|toml|ya?ml|png)(?::\d+(?:-\d+)?)?\b"),
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

/// Evidence markers: a commit SHA, test output, a test command, a file
/// reference, a PR or issue number, or an explicit "evidence:" label.
pub fn has_evidence(text: &str) -> bool {
    evidence_patterns().iter().any(|regex| regex.is_match(text))
        || commit_sha().find_iter(text).any(|found| is_sha(found.as_str()))
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
/// itself declares completion (a `completed` heartbeat).
pub fn map_message(text: &str, transport_declares_done: bool) -> MessageActs {
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
        has_evidence: has_evidence(&normalized),
        hits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acts(text: &str) -> Vec<Act> {
        map_message(text, false).hits.into_iter().map(|hit| hit.act).collect()
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

    #[test]
    fn evidence_requires_a_real_marker() {
        assert!(has_evidence("commit 3fa9c1e"));
        assert!(has_evidence("cargo test --lib: 42 passed"));
        assert!(!has_evidence("Done, everything works."));
        assert!(!has_evidence("deadbeef is not a sha without digits"));
    }

    #[test]
    fn completed_transport_forces_declares_done() {
        let mapped = map_message("", true);
        assert!(mapped.contains(Act::DeclaresDone));
        assert!(!mapped.has_evidence);
    }
}
