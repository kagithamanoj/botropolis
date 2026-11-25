//! Typed output contracts for Botropolis agents.
//!
//! Every agent declares the shape of its answer. The declaration lives
//! in the agent's YAML spec as `output_schema`, naming one of the
//! registered schemas below; when the spec omits it, a sensible default
//! is picked per department. The framework asks the model for JSON
//! matching that schema, validates it, and retries once with the
//! validation errors spelled out when it does not match.
//!
//! Design rule: every schema requires only `summary`. Everything else
//! is optional, so a terse but valid answer never fails validation for
//! being short. Mirrors `botropolis/core/output_schemas.py`.

use serde_json::{Map, Value};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{0}")]
pub struct OutputValidationError(pub String);

/// The validated shape of one agent answer.
#[derive(Debug, Clone, Default)]
pub struct AgentOutput {
    pub summary: String,
    pub details: String,
    pub key_points: Vec<String>,
    pub extra: HashMap<String, Value>,
}

impl AgentOutput {
    /// Render a validated answer as human-readable prose: summary
    /// first, details after.
    pub fn render(&self) -> String {
        if self.details.trim().is_empty() {
            self.summary.trim().to_string()
        } else {
            format!("{}\n\n{}", self.summary.trim(), self.details.trim())
        }
    }

    /// The validated model as a plain JSON map.
    pub fn dump(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        out.insert("summary".to_string(), Value::String(self.summary.clone()));
        out.insert("details".to_string(), Value::String(self.details.clone()));
        out.insert(
            "key_points".to_string(),
            Value::Array(
                self.key_points
                    .iter()
                    .map(|p| Value::String(p.clone()))
                    .collect(),
            ),
        );
        for (key, value) in &self.extra {
            out.insert(key.clone(), value.clone());
        }
        out
    }
}

pub const OUTPUT_SCHEMAS: [&str; 4] = [
    "agent_output",
    "research_output",
    "code_output",
    "recommendation_output",
];

/// Sensible default per department. A spec's `output_schema` field
/// overrides this; unknown names fail fast at agent load time.
pub fn department_output_schema(department: &str) -> &'static str {
    match department {
        "research" => "research_output",
        "code" => "code_output",
        "finance" | "legal" | "health" | "security" | "data" => "recommendation_output",
        _ => "agent_output",
    }
}

/// Resolve a schema name to its canonical name. Raises on unknown
/// names so a typo in a spec fails at load time, not mid-run.
pub fn resolve_output_schema(
    name: Option<&str>,
    department: &str,
) -> Result<&'static str, OutputValidationError> {
    let wanted = name.unwrap_or_else(|| department_output_schema(department));
    if OUTPUT_SCHEMAS.contains(&wanted) {
        Ok(match wanted {
            "research_output" => "research_output",
            "code_output" => "code_output",
            "recommendation_output" => "recommendation_output",
            _ => "agent_output",
        })
    } else {
        Err(OutputValidationError(format!(
            "unknown output_schema '{wanted}'. Available: {:?}",
            OUTPUT_SCHEMAS
        )))
    }
}

/// The JSON schema text for a schema name, as shown to the model.
pub fn schema_json(schema_name: &str) -> String {
    let base = r#"{"type":"object","properties":{"summary":{"type":"string","minLength":1},"details":{"type":"string"},"key_points":{"type":"array","items":{"type":"string"}}},"required":["summary"]}"#;
    match schema_name {
        "research_output" => {
            r#"{"type":"object","properties":{"summary":{"type":"string","minLength":1},"details":{"type":"string"},"key_points":{"type":"array","items":{"type":"string"}},"sources":{"type":"array","items":{"type":"string"}},"open_questions":{"type":"array","items":{"type":"string"}}},"required":["summary"]}"#.to_string()
        }
        "code_output" => {
            r#"{"type":"object","properties":{"summary":{"type":"string","minLength":1},"details":{"type":"string"},"key_points":{"type":"array","items":{"type":"string"}},"files_changed":{"type":"array","items":{"type":"string"}},"tests_passed":{"type":"boolean"}},"required":["summary"]}"#.to_string()
        }
        "recommendation_output" => {
            r#"{"type":"object","properties":{"summary":{"type":"string","minLength":1},"details":{"type":"string"},"key_points":{"type":"array","items":{"type":"string"}},"risks":{"type":"array","items":{"type":"string"}},"recommendation":{"type":"string"}},"required":["summary"]}"#.to_string()
        }
        _ => base.to_string(),
    }
}

/// Build the prompt instruction that asks for schema-shaped JSON.
pub fn format_instruction(schema_name: &str) -> String {
    format!(
        "Your final answer must be a single JSON object matching this schema:\n{}\nReply with ONLY the JSON object. No prose, no fences, no explanation around it.",
        schema_json(schema_name)
    )
}

fn json_candidates(raw: &str) -> Vec<String> {
    let text = raw.trim();
    let mut out = Vec::new();
    if let Some(stripped) = text.strip_prefix("```") {
        let mut lines: Vec<&str> = stripped.lines().collect();
        if !lines.is_empty() {
            lines.remove(0); // opening fence, possibly with a language tag
        }
        if let Some(last) = lines.last() {
            if last.trim().starts_with("```") {
                lines.pop();
            }
        }
        out.push(lines.join("\n").trim().to_string());
    } else {
        out.push(text.to_string());
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if start < end {
            let span = &text[start..=end];
            if span != text {
                out.push(span.to_string());
            }
        }
    }
    out
}

/// Pull the JSON payload out of raw model text.
pub fn extract_json(raw: &str) -> String {
    json_candidates(raw).into_iter().next().unwrap_or_default()
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse raw model text into a validated AgentOutput.
///
/// Raises OutputValidationError when the text is not JSON or does not
/// satisfy the schema. Every error names the schema so logs and repair
/// prompts say what was expected.
pub fn parse_structured_output(
    raw: &str,
    schema_name: &str,
) -> Result<AgentOutput, OutputValidationError> {
    let mut last_error = String::new();
    for candidate in json_candidates(raw) {
        let data: Value = match serde_json::from_str(&candidate) {
            Ok(v) => v,
            Err(e) => {
                last_error = format!("output is not valid JSON for {schema_name}: {e}");
                continue;
            }
        };
        let obj: &Map<String, Value> = match data.as_object() {
            Some(o) => o,
            None => {
                last_error =
                    format!("output JSON for {schema_name} must be an object, not a list or value");
                continue;
            }
        };
        let summary = match obj.get("summary").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                last_error = format!(
                    "output does not match the {schema_name} schema: summary: must be a non-empty string"
                );
                continue;
            }
        };
        let mut out = AgentOutput {
            summary,
            details: obj
                .get("details")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            key_points: string_list(obj.get("key_points")),
            extra: HashMap::new(),
        };
        for (key, value) in obj {
            if !matches!(key.as_str(), "summary" | "details" | "key_points") {
                out.extra.insert(key.clone(), value.clone());
            }
        }
        return Ok(out);
    }
    Err(OutputValidationError(last_error.if_empty_then(|| {
        format!("output is empty, expected {schema_name} JSON")
    })))
}

trait IfEmpty {
    fn if_empty_then(self, f: impl FnOnce() -> String) -> String;
}

impl IfEmpty for String {
    fn if_empty_then(self, f: impl FnOnce() -> String) -> String {
        if self.is_empty() {
            f()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_fenced_json() {
        let out = parse_structured_output(
            r#"{"summary": "done", "details": "all good", "key_points": ["a"]}"#,
            "agent_output",
        )
        .unwrap();
        assert_eq!(out.summary, "done");
        assert_eq!(out.details, "all good");
        assert_eq!(out.key_points, vec!["a"]);
        let fenced =
            parse_structured_output("```json\n{\"summary\": \"fenced\"}\n```", "agent_output")
                .unwrap();
        assert_eq!(fenced.summary, "fenced");
    }

    #[test]
    fn missing_summary_fails() {
        assert!(parse_structured_output(r#"{"details": "x"}"#, "agent_output").is_err());
        assert!(parse_structured_output("not json at all", "agent_output").is_err());
    }

    #[test]
    fn unknown_schema_name_fails() {
        assert!(resolve_output_schema(Some("nope"), "research").is_err());
        assert_eq!(
            resolve_output_schema(None, "research").unwrap(),
            "research_output"
        );
        assert_eq!(resolve_output_schema(None, "ops").unwrap(), "agent_output");
    }
}
