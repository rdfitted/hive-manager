use std::fs;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SECRET_ENV_VARS: &[&str] = &[
    "TYPESAFE_API_KEY",
    "AI_GATEWAY_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
];

pub(super) struct EvidenceWrite {
    pub(super) state_hash: String,
    pub(super) state_ref: String,
}

#[derive(Default)]
struct PythonFloatFormatter;

impl serde_json::ser::Formatter for PythonFloatFormatter {
    fn write_f32<W>(&mut self, writer: &mut W, value: f32) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        write_python_float(writer, value as f64)
    }

    fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        write_python_float(writer, value)
    }
}

fn write_python_float<W>(writer: &mut W, value: f64) -> io::Result<()>
where
    W: ?Sized + io::Write,
{
    let rendered = serde_json::to_string(&value).map_err(io::Error::other)?;
    if rendered == "0.0" || rendered == "-0.0" {
        return writer.write_all(rendered.as_bytes());
    }

    let (negative, unsigned) = rendered
        .strip_prefix('-')
        .map_or((false, rendered.as_str()), |rest| (true, rest));
    let (coefficient, explicit_exponent) = unsigned
        .split_once('e')
        .or_else(|| unsigned.split_once('E'))
        .map_or(Ok((unsigned, 0)), |(coefficient, exponent)| {
            exponent
                .parse::<i32>()
                .map(|exponent| (coefficient, exponent))
                .map_err(io::Error::other)
        })?;
    let integer_digits = coefficient.find('.').unwrap_or(coefficient.len());
    let raw_digits: String = coefficient.chars().filter(|ch| *ch != '.').collect();
    let first_nonzero = raw_digits
        .find(|ch| ch != '0')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid zero float"))?;
    let normalized_exponent = explicit_exponent + integer_digits as i32 - first_nonzero as i32 - 1;
    let significant = raw_digits[first_nonzero..].trim_end_matches('0');
    let significant = if significant.is_empty() {
        "0"
    } else {
        significant
    };
    let sign = if negative { "-" } else { "" };

    let python = if !(-4..16).contains(&normalized_exponent) {
        let mut chars = significant.chars();
        let first = chars.next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "significant digits are empty")
        })?;
        let rest: String = chars.collect();
        let mantissa = if rest.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{rest}")
        };
        format!("{sign}{mantissa}e{normalized_exponent:+03}")
    } else if normalized_exponent >= 0 {
        let decimal_at = normalized_exponent as usize + 1;
        if significant.len() <= decimal_at {
            format!(
                "{sign}{significant}{}.0",
                "0".repeat(decimal_at - significant.len())
            )
        } else {
            format!(
                "{sign}{}.{}",
                &significant[..decimal_at],
                &significant[decimal_at..]
            )
        }
    } else {
        format!(
            "{sign}0.{}{significant}",
            "0".repeat((-normalized_exponent - 1) as usize)
        )
    };
    writer.write_all(python.as_bytes())
}

fn secret_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"sk-[A-Za-z0-9_-]{20,}").expect("valid secret regex"))
}

fn bearer_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)bearer\s+[A-Za-z0-9._-]{16,}").expect("valid bearer regex")
    })
}

pub(super) fn scrub(text: &str) -> String {
    let mut clean = text.to_string();
    for name in SECRET_ENV_VARS {
        if let Ok(value) = std::env::var(name) {
            if value.len() >= 8 && clean.contains(&value) {
                clean = clean.replace(&value, &format!("[REDACTED:{name}]"));
            }
        }
    }
    let clean = secret_pattern().replace_all(&clean, "[REDACTED]");
    bearer_pattern()
        .replace_all(&clean, "[REDACTED]")
        .into_owned()
}

pub(super) fn scrub_value(value: &Value) -> io::Result<Value> {
    let serialized = serde_json::to_string(value).map_err(io::Error::other)?;
    serde_json::from_str(&scrub(&serialized)).map_err(io::Error::other)
}

pub(super) fn canonical_json(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, PythonFloatFormatter);
    value.serialize(&mut serializer).map_err(io::Error::other)?;
    Ok(bytes)
}

pub(super) fn sha256_of(value: &Value) -> io::Result<String> {
    let canonical = canonical_json(value)?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
}

pub(super) fn write_evidence(
    ledger_path: &Path,
    decision_id: &str,
    observations: &Value,
) -> io::Result<EvidenceWrite> {
    if decision_id.is_empty()
        || decision_id.contains('/')
        || decision_id.contains('\\')
        || decision_id == "."
        || decision_id == ".."
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "decision id is not safe for an evidence filename",
        ));
    }

    let clean = scrub_value(observations)?;
    let canonical = canonical_json(&clean)?;
    let state_hash = sha256_of(&clean)?;
    let parent = ledger_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ledger path has no parent"))?;
    let evidence_dir = parent.join("evidence");
    fs::create_dir_all(&evidence_dir)?;
    fs::write(evidence_dir.join(format!("{decision_id}.json")), canonical)?;

    Ok(EvidenceWrite {
        state_hash,
        state_ref: format!("evidence/{decision_id}.json"),
    })
}
