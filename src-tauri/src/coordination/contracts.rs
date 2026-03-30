use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SprintContract {
    pub milestone_name: String,
    pub acceptance_criteria: Vec<ContractCriterion>,
    pub pass_threshold: Vec<String>,
    pub raw_markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractCriterion {
    pub number: u16,
    pub category: Option<String>,
    pub description: String,
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
            criteria.push(parse_criterion_line(line)?);
            continue;
        }

        if in_threshold {
            if let Some(item) = line.strip_prefix("- ") {
                pass_threshold.push(item.trim().to_string());
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

    Ok(SprintContract {
        milestone_name,
        acceptance_criteria: criteria,
        pass_threshold,
        raw_markdown: markdown.to_string(),
    })
}

fn parse_criterion_line(line: &str) -> Result<ContractCriterion, ContractParseError> {
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

    let (category, description) = if let Some(rest) = remainder.strip_prefix('[') {
        let (category, desc) = rest
            .split_once(']')
            .ok_or_else(|| ContractParseError::InvalidCriterion(line.to_string()))?;
        let description = desc.trim();
        if description.is_empty() {
            return Err(ContractParseError::InvalidCriterion(line.to_string()));
        }
        (Some(category.trim().to_string()), description.to_string())
    } else {
        (None, remainder.to_string())
    };

    Ok(ContractCriterion {
        number,
        category,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_sprint_contract, ContractParseError};

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
}
