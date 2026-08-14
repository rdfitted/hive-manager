//! Order-independent disagreement policies for review subgraphs.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{AuthorityScope, ContextBoundary, SignalClass};

/// Verification is a named signal with an explicit judgment class and context
/// boundary. Role defaults must not silently fill any of these plan-time facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationDuty {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_class: Option<SignalClass>,
    pub context_boundary: ContextBoundary,
}

impl Default for VerificationDuty {
    fn default() -> Self {
        Self {
            signal_name: None,
            signal_class: None,
            context_boundary: ContextBoundary::Full,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdjudicationPolicy {
    Consensus { required: usize },
    Escalate,
    HumanGate,
    BothAreFindings,
}

/// The role named by a review subgraph, including the authority declaration
/// that permits it to adjudicate. An absent role is never replaced by Queen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredAdjudicator {
    pub role_id: String,
    pub authority: AuthorityScope,
}

impl DeclaredAdjudicator {
    pub fn new(role_id: impl Into<String>) -> Self {
        Self {
            role_id: role_id.into(),
            authority: AuthorityScope {
                may_adjudicate: true,
                ..AuthorityScope::default()
            },
        }
    }
}

/// Policy plus the role that holds the decision authority for one review
/// subgraph. `adjudicator` remains optional so PlanReady can report absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjudicationDeclaration {
    pub policy: AdjudicationPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjudicator: Option<DeclaredAdjudicator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceVerdictValue {
    Pass,
    Fail,
}

/// An immutable source verdict. Every one is retained alongside, not replaced
/// by, the separate adjudication record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVerdict {
    pub source_id: String,
    pub verdict: SourceVerdictValue,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdjudicationResolution {
    ConsensusPass,
    ConsensusFail,
    ConsensusUnresolved {
        required: usize,
        pass_count: usize,
        fail_count: usize,
    },
    Escalated { role_id: String },
    HumanGate,
    Findings { source_ids: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjudicationRecord {
    pub policy: AdjudicationPolicy,
    pub adjudicator: DeclaredAdjudicator,
    pub source_verdicts: Vec<SourceVerdict>,
    pub resolution: AdjudicationResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdjudicationError {
    MissingAdjudicator,
    AdjudicatorLacksAuthority { role_id: String },
    TooFewVerdicts,
    DuplicateSourceVerdict { source_id: String },
    NoContradiction,
    InvalidConsensusThreshold { required: usize, verdicts: usize },
}

impl fmt::Display for AdjudicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAdjudicator => write!(formatter, "review disagreement has no adjudicator"),
            Self::AdjudicatorLacksAuthority { role_id } => write!(
                formatter,
                "declared adjudicator role {role_id} lacks adjudication authority"
            ),
            Self::TooFewVerdicts => write!(formatter, "disagreement needs at least two verdicts"),
            Self::DuplicateSourceVerdict { source_id } => {
                write!(formatter, "duplicate source verdict {source_id}")
            }
            Self::NoContradiction => write!(formatter, "verdict set does not contradict"),
            Self::InvalidConsensusThreshold { required, verdicts } => write!(
                formatter,
                "consensus threshold {required} is invalid for {verdicts} verdicts"
            ),
        }
    }
}

impl Error for AdjudicationError {}

/// Resolve a contradictory verdict set without consulting arrival order.
pub fn adjudicate_contradiction(
    declaration: &AdjudicationDeclaration,
    verdicts: &[SourceVerdict],
) -> Result<AdjudicationRecord, AdjudicationError> {
    let adjudicator = declaration
        .adjudicator
        .as_ref()
        .ok_or(AdjudicationError::MissingAdjudicator)?;
    if adjudicator.role_id.trim().is_empty() || !adjudicator.authority.may_adjudicate {
        return Err(AdjudicationError::AdjudicatorLacksAuthority {
            role_id: adjudicator.role_id.clone(),
        });
    }
    if verdicts.len() < 2 {
        return Err(AdjudicationError::TooFewVerdicts);
    }

    let mut source_ids = BTreeSet::new();
    for verdict in verdicts {
        if !source_ids.insert(verdict.source_id.as_str()) {
            return Err(AdjudicationError::DuplicateSourceVerdict {
                source_id: verdict.source_id.clone(),
            });
        }
    }
    let pass_count = verdicts
        .iter()
        .filter(|verdict| verdict.verdict == SourceVerdictValue::Pass)
        .count();
    let fail_count = verdicts.len() - pass_count;
    if pass_count == 0 || fail_count == 0 {
        return Err(AdjudicationError::NoContradiction);
    }

    let resolution = match &declaration.policy {
        AdjudicationPolicy::Consensus { required } => {
            if *required == 0 || *required > verdicts.len() {
                return Err(AdjudicationError::InvalidConsensusThreshold {
                    required: *required,
                    verdicts: verdicts.len(),
                });
            }
            match (pass_count >= *required, fail_count >= *required) {
                (true, false) => AdjudicationResolution::ConsensusPass,
                (false, true) => AdjudicationResolution::ConsensusFail,
                // If both sides meet a permissive threshold, contradiction is
                // not resolved by privileging whichever branch was checked first.
                (true, true) | (false, false) => AdjudicationResolution::ConsensusUnresolved {
                    required: *required,
                    pass_count,
                    fail_count,
                },
            }
        }
        AdjudicationPolicy::Escalate => AdjudicationResolution::Escalated {
            role_id: adjudicator.role_id.clone(),
        },
        AdjudicationPolicy::HumanGate => AdjudicationResolution::HumanGate,
        AdjudicationPolicy::BothAreFindings => AdjudicationResolution::Findings {
            source_ids: verdicts
                .iter()
                .map(|verdict| verdict.source_id.clone())
                .collect(),
        },
    };
    let mut source_verdicts = verdicts.to_vec();
    source_verdicts.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    let resolution = match resolution {
        AdjudicationResolution::Findings { .. } => AdjudicationResolution::Findings {
            source_ids: source_verdicts
                .iter()
                .map(|verdict| verdict.source_id.clone())
                .collect(),
        },
        other => other,
    };
    Ok(AdjudicationRecord {
        policy: declaration.policy.clone(),
        adjudicator: adjudicator.clone(),
        source_verdicts,
        resolution,
    })
}
