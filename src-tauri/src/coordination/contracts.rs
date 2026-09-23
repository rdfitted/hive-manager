use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SprintContract {
    pub milestone_name: String,
    pub acceptance_criteria: Vec<ContractCriterion>,
    pub pass_threshold: Vec<String>,
    #[serde(default)]
    pub threshold_policy: ThresholdPolicy,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub raw_markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractCriterion {
    pub number: u16,
    pub category: Option<String>,
    #[serde(default)]
    pub kind: CriterionKind,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum CriterionKind {
    PassFail,
    Scored {
        min: u8,
        max: u8,
        floor: Option<u8>,
    },
    Measured {
        metric: String,
        op: CompareOp,
        target: f64,
    },
    #[default]
    Unspecified,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompareOp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ThresholdPolicy {
    Rules {
        require_all_pass_fail: bool,
        scored_mean: Option<ScoredMeanThreshold>,
        fail_scored_below_floor: bool,
    },
    Prose(Vec<String>),
}

impl Default for ThresholdPolicy {
    fn default() -> Self {
        Self::Prose(Vec::new())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ScoredMeanThreshold {
    pub minimum: f64,
    pub scale: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CriterionResult {
    pub kind: CriterionKind,
    pub value: CriterionValue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CriterionValue {
    PassFail(bool),
    Scored(f64),
    Measured(f64),
    Unspecified,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    #[default]
    Undetermined,
}

impl SprintContract {
    #[cfg(test)]
    pub fn criterion(&self, number: u16) -> Option<&ContractCriterion> {
        self.acceptance_criteria
            .iter()
            .find(|criterion| criterion.number == number)
    }
}

#[derive(Debug, Error)]
pub enum ContractParseError {
    #[error("missing contract title header")]
    MissingTitle,
    #[error("missing acceptance criteria section")]
    MissingAcceptanceCriteria,
    #[error("missing pass threshold section")]
    MissingPassThreshold,
    #[error("invalid criterion line: {0}")]
    InvalidCriterion(String),
    #[error("contract must contain at least one acceptance criterion")]
    NoCriteria,
    #[error("contract must contain at least one pass threshold bullet")]
    NoThresholds,
}

pub fn parse_sprint_contract(markdown: &str) -> Result<SprintContract, ContractParseError> {
    let mut milestone_name = None;
    let mut in_acceptance = false;
    let mut in_threshold = false;
    let mut criteria = Vec::new();
    let mut pass_threshold = Vec::new();
    let mut warnings = Vec::new();

    for raw_line in markdown.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(title) = line.strip_prefix("# Sprint Contract:") {
            milestone_name = Some(title.trim().to_string());
            in_acceptance = false;
            in_threshold = false;
            continue;
        }
        if line.eq_ignore_ascii_case("## Acceptance Criteria") {
            in_acceptance = true;
            in_threshold = false;
            continue;
        }
        if line.eq_ignore_ascii_case("## Pass Threshold") {
            in_acceptance = false;
            in_threshold = true;
            continue;
        }
        if line.starts_with('#') {
            in_acceptance = false;
            in_threshold = false;
            continue;
        }

        if in_acceptance {
            let (criterion, warning) = parse_criterion_line(line)?;
            if let Some(warning) = warning {
                warnings.push(warning);
            }
            criteria.push(criterion);
            continue;
        }

        if in_threshold {
            if let Some(item) = line.strip_prefix("- ") {
                let trimmed = item.trim();
                if !trimmed.is_empty() {
                    pass_threshold.push(trimmed.to_string());
                }
            }
        }
    }

    let milestone_name = milestone_name.ok_or(ContractParseError::MissingTitle)?;
    if criteria.is_empty() {
        return Err(if markdown.to_lowercase().contains("## acceptance criteria") {
            ContractParseError::NoCriteria
        } else {
            ContractParseError::MissingAcceptanceCriteria
        });
    }
    if pass_threshold.is_empty() {
        return Err(if markdown.to_lowercase().contains("## pass threshold") {
            ContractParseError::NoThresholds
        } else {
            ContractParseError::MissingPassThreshold
        });
    }

    let threshold_policy = parse_threshold_policy(&pass_threshold);

    Ok(SprintContract {
        milestone_name,
        acceptance_criteria: criteria,
        pass_threshold,
        threshold_policy,
        warnings,
        raw_markdown: markdown.to_string(),
    })
}

fn parse_criterion_line(
    line: &str,
) -> Result<(ContractCriterion, Option<String>), ContractParseError> {
    let (number_part, remainder) = line
        .split_once('.')
        .ok_or_else(|| ContractParseError::InvalidCriterion(line.to_string()))?;
    let number = number_part
        .trim()
        .parse::<u16>()
        .map_err(|_| ContractParseError::InvalidCriterion(line.to_string()))?;
    let remainder = remainder.trim();
    if remainder.is_empty() {
        return Err(ContractParseError::InvalidCriterion(line.to_string()));
    }

    let (category, kind, warning, description) = if let Some(rest) = remainder.strip_prefix('[') {
        let (category, desc) = rest
            .split_once(']')
            .ok_or_else(|| ContractParseError::InvalidCriterion(line.to_string()))?;
        let category = category.trim();
        let description = desc.trim();
        if description.is_empty() {
            return Err(ContractParseError::InvalidCriterion(line.to_string()));
        }
        let category = if category.is_empty() {
            None
        } else {
            Some(category.to_string())
        };
        let (kind, warning) = parse_criterion_kind(category.as_deref(), number);
        (category, kind, warning, description.to_string())
    } else {
        (
            None,
            CriterionKind::Unspecified,
            None,
            remainder.to_string(),
        )
    };

    Ok((
        ContractCriterion {
            number,
            category,
            kind,
            description,
        },
        warning,
    ))
}

fn parse_criterion_kind(category: Option<&str>, number: u16) -> (CriterionKind, Option<String>) {
    let Some(category) = category else {
        return (CriterionKind::Unspecified, None);
    };
    let normalized = category.trim().to_ascii_lowercase();

    if normalized == "func" {
        return (CriterionKind::PassFail, None);
    }

    if let Some(specification) = normalized
        .strip_prefix("design ")
        .or_else(|| normalized.strip_prefix("scored "))
    {
        if let Some((min, max, floor)) = parse_scored_kind(specification) {
            return (CriterionKind::Scored { min, max, floor }, None);
        }
    }

    if let Some(specification) = normalized
        .strip_prefix("perf ")
        .or_else(|| normalized.strip_prefix("measured "))
    {
        if let Some((metric, op, target)) = parse_measured_kind(specification) {
            return (
                CriterionKind::Measured { metric, op, target },
                None,
            );
        }
    }

    (
        CriterionKind::Unspecified,
        Some(format!(
            "criterion {number} has unrecognised kind bracket [{category}]"
        )),
    )
}

fn parse_scored_kind(specification: &str) -> Option<(u8, u8, Option<u8>)> {
    let mut parts = specification.split_whitespace();
    let (min, max) = parts.next()?.split_once('-')?;
    let min = min.parse::<u8>().ok()?;
    let max = max.parse::<u8>().ok()?;
    if min > max {
        return None;
    }

    let floor = match (parts.next(), parts.next(), parts.next()) {
        (None, None, None) => None,
        (Some("floor"), Some(value), None) => Some(value.parse::<u8>().ok()?),
        _ => return None,
    };
    if floor.is_some_and(|floor| floor < min || floor > max) {
        return None;
    }
    Some((min, max, floor))
}

fn parse_measured_kind(specification: &str) -> Option<(String, CompareOp, f64)> {
    let specification = specification.trim_matches(|character: char| {
        character.is_ascii_whitespace() || matches!(character, ':' | '(' | ')')
    });
    let operators = [
        ("<=", CompareOp::Le),
        (">=", CompareOp::Ge),
        ("==", CompareOp::Eq),
        ("=", CompareOp::Eq),
        ("<", CompareOp::Lt),
        (">", CompareOp::Gt),
    ];
    for (token, op) in operators {
        let Some((metric, target)) = specification.split_once(token) else {
            continue;
        };
        let metric = metric.trim().trim_end_matches(':');
        let target = target.trim().parse::<f64>().ok()?;
        if !metric.is_empty() && target.is_finite() {
            return Some((metric.to_string(), op, target));
        }
    }
    None
}

fn parse_threshold_policy(lines: &[String]) -> ThresholdPolicy {
    let mut require_all_pass_fail = false;
    let mut scored_mean = None;
    let mut fail_scored_below_floor = false;

    for line in lines {
        let normalized = line.trim().to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "all pass/fail criteria must pass" | "all func criteria must pass"
        ) {
            require_all_pass_fail = true;
            continue;
        }

        if let Some(rule) = parse_scored_mean_threshold(&normalized) {
            scored_mean = Some(rule);
            continue;
        }

        if is_scored_floor_rule(&normalized) {
            fail_scored_below_floor = true;
            continue;
        }

        return ThresholdPolicy::Prose(lines.to_vec());
    }

    ThresholdPolicy::Rules {
        require_all_pass_fail,
        scored_mean,
        fail_scored_below_floor,
    }
}

fn is_scored_floor_rule(line: &str) -> bool {
    if matches!(
        line,
        "any scored criterion below its floor fails"
            | "any single scored criterion below its floor is an automatic fail"
    ) {
        return true;
    }

    let Some(remainder) = line.strip_prefix("any single scored criterion < ") else {
        return false;
    };
    let Some(ratio) = remainder.strip_suffix(" is an automatic fail") else {
        return false;
    };
    let Some((minimum, scale)) = ratio.split_once('/') else {
        return false;
    };
    minimum.trim().parse::<u8>().is_ok() && scale.trim().parse::<u8>().is_ok()
}

fn parse_scored_mean_threshold(line: &str) -> Option<ScoredMeanThreshold> {
    let remainder = line
        .strip_prefix("scored criteria average >= ")
        .or_else(|| line.strip_prefix("scored criteria must average >= "))?;
    let (minimum, scale) = remainder.split_once('/')?;
    let minimum = minimum.trim().parse::<f64>().ok()?;
    let scale = scale.trim().parse::<f64>().ok()?;
    if minimum.is_finite() && scale.is_finite() && scale > 0.0 && minimum <= scale {
        Some(ScoredMeanThreshold { minimum, scale })
    } else {
        None
    }
}

pub fn criterion_verdict(result: &CriterionResult) -> Verdict {
    match (&result.kind, result.value) {
        (CriterionKind::PassFail, CriterionValue::PassFail(passed)) => {
            if passed {
                Verdict::Pass
            } else {
                Verdict::Fail
            }
        }
        (CriterionKind::Scored { min, max, .. }, CriterionValue::Scored(value))
            if value.is_finite() && value >= f64::from(*min) && value <= f64::from(*max) =>
        {
            Verdict::Pass
        }
        (
            CriterionKind::Measured { op, target, .. },
            CriterionValue::Measured(value),
        ) if value.is_finite() && target.is_finite() => {
            let passed = match op {
                CompareOp::Lt => value < *target,
                CompareOp::Le => value <= *target,
                CompareOp::Eq => value == *target,
                CompareOp::Ge => value >= *target,
                CompareOp::Gt => value > *target,
            };
            if passed {
                Verdict::Pass
            } else {
                Verdict::Fail
            }
        }
        _ => Verdict::Undetermined,
    }
}

/// Pure typed-contract evaluation.
///
/// Measured criteria gate even when `require_all_pass_fail` is false. An
/// unspecified criterion, a missing result, or a scored value outside its
/// declared `min..=max` range produces [`Verdict::Undetermined`] unless another
/// result already makes the policy fail.
pub fn evaluate(policy: &ThresholdPolicy, results: &[CriterionResult]) -> Verdict {
    let ThresholdPolicy::Rules {
        require_all_pass_fail,
        scored_mean,
        fail_scored_below_floor,
    } = policy
    else {
        return Verdict::Undetermined;
    };
    if results.is_empty() {
        return Verdict::Undetermined;
    }
    if scored_mean.is_some_and(|threshold| {
        !threshold.minimum.is_finite()
            || !threshold.scale.is_finite()
            || threshold.scale <= 0.0
    }) {
        return Verdict::Undetermined;
    }

    let mut saw_undetermined = false;
    let mut scored_ratios = Vec::new();
    for result in results {
        match (&result.kind, result.value) {
            (CriterionKind::PassFail, _) if *require_all_pass_fail => {
                match criterion_verdict(result) {
                    Verdict::Pass => {}
                    Verdict::Fail => return Verdict::Fail,
                    Verdict::Undetermined => saw_undetermined = true,
                }
            }
            (
                CriterionKind::Scored { max, floor, .. },
                CriterionValue::Scored(value),
            ) if value.is_finite() && *max > 0 => {
                if *fail_scored_below_floor
                    && floor.is_some_and(|floor| value < f64::from(floor))
                {
                    return Verdict::Fail;
                }
                if criterion_verdict(result) == Verdict::Undetermined {
                    saw_undetermined = true;
                } else {
                    scored_ratios.push(value / f64::from(*max));
                }
            }
            (CriterionKind::Scored { .. }, _) => saw_undetermined = true,
            (CriterionKind::Measured { .. }, _) => match criterion_verdict(result) {
                Verdict::Pass => {}
                Verdict::Fail => return Verdict::Fail,
                Verdict::Undetermined => saw_undetermined = true,
            },
            (CriterionKind::Unspecified, _) => saw_undetermined = true,
            _ => {}
        }
    }

    if let Some(scored_mean) = scored_mean {
        if scored_ratios.is_empty() {
            saw_undetermined = true;
        } else {
            let mean = scored_ratios.iter().sum::<f64>() / scored_ratios.len() as f64;
            if mean < scored_mean.minimum / scored_mean.scale {
                return Verdict::Fail;
            }
        }
    }

    if saw_undetermined {
        Verdict::Undetermined
    } else {
        Verdict::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate, parse_sprint_contract, CompareOp, ContractParseError, CriterionKind,
        CriterionResult, CriterionValue, ScoredMeanThreshold, ThresholdPolicy, Verdict,
    };

    #[test]
    fn parses_numbered_contract_criteria_and_thresholds() {
        let contract = parse_sprint_contract(
            r#"# Sprint Contract: Dashboard QA

## Acceptance Criteria
1. [FUNC] Dashboard loads for authenticated users
2. [A11Y] Keyboard users can reach every action
3. [PERF] First contentful paint stays under 2.5s

## Pass Threshold
- All FUNC criteria must PASS
- Scored criteria average >= 7/10
"#,
        )
        .unwrap();

        assert_eq!(contract.milestone_name, "Dashboard QA");
        assert_eq!(contract.acceptance_criteria.len(), 3);
        assert_eq!(
            contract.criterion(2).unwrap().category.as_deref(),
            Some("A11Y")
        );
        assert_eq!(contract.pass_threshold.len(), 2);
    }

    #[test]
    fn rejects_contract_without_thresholds() {
        let err = parse_sprint_contract(
            r#"# Sprint Contract: Missing Threshold

## Acceptance Criteria
1. [FUNC] Something works
"#,
        )
        .unwrap_err();

        assert!(matches!(err, ContractParseError::MissingPassThreshold));
    }

    #[test]
    fn treats_empty_category_brackets_as_uncategorized() {
        let contract = parse_sprint_contract(
            r#"# Sprint Contract: Empty Category

## Acceptance Criteria
1. [] Description stays valid

## Pass Threshold
- Criterion 1 must PASS
"#,
        )
        .unwrap();

        let criterion = contract.criterion(1).unwrap();
        assert_eq!(criterion.category, None);
        assert_eq!(criterion.description, "Description stays valid");
    }

    #[test]
    fn rejects_contract_with_only_empty_threshold_bullets() {
        let err = parse_sprint_contract(
            r#"# Sprint Contract: Empty Thresholds

## Acceptance Criteria
1. [FUNC] Something works

## Pass Threshold
- 
-    
"#,
        )
        .unwrap_err();

        assert!(matches!(err, ContractParseError::NoThresholds));
    }

    fn contract_with_criterion(category: &str) -> String {
        format!(
            "# Sprint Contract: Typed\n\n## Acceptance Criteria\n1. [{category}] It works\n\n## Pass Threshold\n- All typed criteria must pass\n"
        )
    }

    #[test]
    fn parses_pass_fail_criterion_kind() {
        let markdown = contract_with_criterion("FUNC");
        let contract = parse_sprint_contract(&markdown).unwrap();

        assert_eq!(
            contract.criterion(1).unwrap().kind,
            CriterionKind::PassFail
        );
        assert!(contract.warnings.is_empty());
    }

    #[test]
    fn parses_scored_criterion_kind() {
        let markdown = contract_with_criterion("DESIGN 1-10 floor 5");
        let contract = parse_sprint_contract(&markdown).unwrap();

        assert_eq!(
            contract.criterion(1).unwrap().kind,
            CriterionKind::Scored {
                min: 1,
                max: 10,
                floor: Some(5),
            }
        );
        assert!(contract.warnings.is_empty());
    }

    #[test]
    fn parses_all_supported_threshold_rules() {
        let markdown = r#"# Sprint Contract: Typed threshold

## Acceptance Criteria
1. [FUNC] It works
2. [DESIGN 1-10 floor 5] It looks right

## Pass Threshold
- All pass/fail criteria must PASS
- Scored criteria average >= 7/10
- Any scored criterion below its floor fails
"#;
        let contract = parse_sprint_contract(markdown).unwrap();

        assert_eq!(
            contract.threshold_policy,
            ThresholdPolicy::Rules {
                require_all_pass_fail: true,
                scored_mean: Some(ScoredMeanThreshold {
                    minimum: 7.0,
                    scale: 10.0,
                }),
                fail_scored_below_floor: true,
            }
        );
    }

    #[test]
    fn parses_measured_criterion_kind() {
        let markdown = contract_with_criterion("PERF lighthouse >= 80");
        let contract = parse_sprint_contract(&markdown).unwrap();

        assert_eq!(
            contract.criterion(1).unwrap().kind,
            CriterionKind::Measured {
                metric: "lighthouse".to_string(),
                op: CompareOp::Ge,
                target: 80.0,
            }
        );
        assert!(contract.warnings.is_empty());
    }

    #[test]
    fn unrecognised_kind_is_unspecified_with_a_warning() {
        let markdown = contract_with_criterion("DESIGN someday");
        let contract = parse_sprint_contract(&markdown).unwrap();

        assert_eq!(
            contract.criterion(1).unwrap().kind,
            CriterionKind::Unspecified
        );
        assert_eq!(contract.warnings.len(), 1);
        assert!(contract.warnings[0].contains("criterion 1"));
    }

    #[test]
    fn untyped_contract_preserves_raw_bytes_and_uses_prose_threshold() {
        let markdown = "# Sprint Contract: Legacy\r\n\r\n## Acceptance Criteria\r\n1. No kind bracket\r\n\r\n## Pass Threshold\r\n- Human judgment applies\r\n";
        let contract = parse_sprint_contract(markdown).unwrap();

        assert_eq!(contract.raw_markdown.as_bytes(), markdown.as_bytes());
        assert_eq!(
            contract.criterion(1).unwrap().kind,
            CriterionKind::Unspecified
        );
        assert_eq!(
            contract.threshold_policy,
            ThresholdPolicy::Prose(vec!["Human judgment applies".to_string()])
        );
        assert!(contract.warnings.is_empty());
    }

    #[test]
    fn evaluate_applies_typed_kinds_and_thresholds() {
        let results = vec![
            CriterionResult {
                kind: CriterionKind::PassFail,
                value: CriterionValue::PassFail(true),
            },
            CriterionResult {
                kind: CriterionKind::Scored {
                    min: 1,
                    max: 10,
                    floor: Some(5),
                },
                value: CriterionValue::Scored(7.0),
            },
            CriterionResult {
                kind: CriterionKind::Measured {
                    metric: "latency".to_string(),
                    op: CompareOp::Lt,
                    target: 250.0,
                },
                value: CriterionValue::Measured(249.0),
            },
        ];

        assert_eq!(
            evaluate(
                &ThresholdPolicy::Rules {
                    require_all_pass_fail: true,
                    scored_mean: Some(ScoredMeanThreshold {
                        minimum: 7.0,
                        scale: 10.0,
                    }),
                    fail_scored_below_floor: true,
                },
                &results,
            ),
            Verdict::Pass
        );
        assert_eq!(
            evaluate(
                &ThresholdPolicy::Prose(vec!["Human judgment".to_string()]),
                &results,
            ),
            Verdict::Undetermined
        );
    }

    #[test]
    fn invalid_scored_mean_scale_or_minimum_is_undetermined() {
        let scored = [CriterionResult {
            kind: CriterionKind::Scored {
                min: 0,
                max: 10,
                floor: None,
            },
            value: CriterionValue::Scored(10.0),
        }];
        for (minimum, scale) in [
            (0.0, 0.0),
            (0.0, -1.0),
            (0.0, f64::NAN),
            (f64::NAN, 10.0),
            (f64::INFINITY, 10.0),
        ] {
            assert_eq!(
                evaluate(
                    &ThresholdPolicy::Rules {
                        require_all_pass_fail: false,
                        scored_mean: Some(ScoredMeanThreshold { minimum, scale }),
                        fail_scored_below_floor: false,
                    },
                    &scored,
                ),
                Verdict::Undetermined,
                "minimum={minimum}, scale={scale}"
            );
        }
    }

    #[test]
    fn evaluate_is_total_and_deterministic_over_small_domain() {
        let kinds = [
            CriterionKind::PassFail,
            CriterionKind::Scored {
                min: 0,
                max: 2,
                floor: Some(1),
            },
            CriterionKind::Measured {
                metric: "value".to_string(),
                op: CompareOp::Lt,
                target: 1.0,
            },
            CriterionKind::Measured {
                metric: "value".to_string(),
                op: CompareOp::Le,
                target: 1.0,
            },
            CriterionKind::Measured {
                metric: "value".to_string(),
                op: CompareOp::Eq,
                target: 1.0,
            },
            CriterionKind::Measured {
                metric: "value".to_string(),
                op: CompareOp::Ge,
                target: 1.0,
            },
            CriterionKind::Measured {
                metric: "value".to_string(),
                op: CompareOp::Gt,
                target: 1.0,
            },
            CriterionKind::Unspecified,
        ];
        let values = [
            CriterionValue::PassFail(false),
            CriterionValue::PassFail(true),
            CriterionValue::Scored(0.0),
            CriterionValue::Scored(1.0),
            CriterionValue::Scored(2.0),
            CriterionValue::Scored(f64::NAN),
            CriterionValue::Measured(0.0),
            CriterionValue::Measured(1.0),
            CriterionValue::Measured(2.0),
            CriterionValue::Measured(f64::INFINITY),
            CriterionValue::Unspecified,
        ];
        let policies = [
            ThresholdPolicy::Rules {
                require_all_pass_fail: true,
                scored_mean: None,
                fail_scored_below_floor: false,
            },
            ThresholdPolicy::Rules {
                require_all_pass_fail: false,
                scored_mean: Some(ScoredMeanThreshold {
                    minimum: 1.0,
                    scale: 2.0,
                }),
                fail_scored_below_floor: true,
            },
            ThresholdPolicy::Prose(vec!["manual".to_string()]),
        ];

        let domain = kinds
            .iter()
            .flat_map(|kind| {
                values.into_iter().map(|value| CriterionResult {
                    kind: kind.clone(),
                    value,
                })
            })
            .collect::<Vec<_>>();

        for policy in &policies {
            assert_evaluation_properties(policy, &[]);
            for result in &domain {
                assert_evaluation_properties(policy, std::slice::from_ref(result));
            }
            for first in &domain {
                for second in &domain {
                    assert_evaluation_properties(policy, &[first.clone(), second.clone()]);
                }
            }
        }

        let mut state = 0x8f3d_9b17_c6a4_2e51_u64;
        for length in 3..=6 {
            for _case in 0..64 {
                let generated = (0..length)
                    .map(|_| {
                        state = state
                            .wrapping_mul(6_364_136_223_846_793_005)
                            .wrapping_add(1_442_695_040_888_963_407);
                        domain[(state as usize) % domain.len()].clone()
                    })
                    .collect::<Vec<_>>();
                for policy in &policies {
                    assert_evaluation_properties(policy, &generated);
                }
            }
        }
    }

    fn assert_evaluation_properties(policy: &ThresholdPolicy, results: &[CriterionResult]) {
        let verdict = evaluate(policy, results);
        assert_eq!(verdict, evaluate(policy, results), "evaluation must be deterministic");

        let mut reversed = results.to_vec();
        reversed.reverse();
        assert_eq!(
            verdict,
            evaluate(policy, &reversed),
            "evaluation must be independent of result order"
        );

        let mut rotated = results.to_vec();
        if !rotated.is_empty() {
            rotated.rotate_left(1);
        }
        assert_eq!(
            verdict,
            evaluate(policy, &rotated),
            "evaluation must be independent of result rotation"
        );

        if matches!(policy, ThresholdPolicy::Prose(_)) || results.is_empty() {
            assert_eq!(verdict, Verdict::Undetermined);
        }

        if matches!(
            policy,
            ThresholdPolicy::Rules {
                require_all_pass_fail: true,
                ..
            }
        ) && results.iter().any(|result| {
            matches!(
                (&result.kind, result.value),
                (CriterionKind::PassFail, CriterionValue::PassFail(false))
            )
        }) {
            assert_eq!(
                verdict,
                Verdict::Fail,
                "a required failed pass/fail criterion must fail the policy"
            );
        }
    }
}
