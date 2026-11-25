//! A separate judge for contracted runs: the builder/judge split.
//!
//! The tool loop's verify pass is the same agent checking its own
//! work. The judge is different: after the machine evidence checks
//! pass, the CEO asks another agent, the Reviewer (Sofia), to judge
//! the final report against the contract's done_when and exclusions,
//! in one single-shot call.
//!
//! The judge is a direct model call, not an agent run: it never goes
//! through the tool loop, never consults, and never recurses into
//! judging. A failed verdict marks the report "unverified" exactly
//! like a failed machine check, with the judge's reason attached as
//! evidence. A pass leaves the status as the machine checks left it.
//!
//! Degradation follows the verify pass: if the judge call errors, if
//! there is no reviewer agent, or if the reply cannot be parsed, the
//! judge is recorded as skipped and the machine-check result stands.
//! The judge never breaks a run. Mirrors `botropolis/core/judge.py`.

use serde_json::{json, Value};

use crate::models::ChatMessage;
use crate::registry::AgentRegistry;
use crate::schemas::Contract;

/// The judge is the Reviewer: the agent whose whole job is reading
/// work skeptically before it ships.
pub const JUDGE_AGENT_NAME: &str = "reviewer";

/// The judge reads the summary plus per-agent outputs, capped so the
/// prompt stays small.
pub const REPORT_TEXT_CHARS: usize = 4000;

/// The judging message: contract terms, the report, what to return.
/// The judge is told plainly it is not the builder.
pub fn judge_prompt(contract: &Contract, report_text: &str) -> String {
    let mut lines = vec![
        "You are the judge for this run, not the builder. A different agent did the work below; your job is to say whether it met the contract it was given.".to_string(),
        String::new(),
        format!("Contract goal: {}", contract.goal),
    ];
    if !contract.done_when.is_empty() {
        lines.push("Done when:".to_string());
        for item in &contract.done_when {
            lines.push(format!("- {item}"));
        }
    }
    if !contract.exclusions.is_empty() {
        lines.push("Out of scope, must not appear in the work:".to_string());
        for item in &contract.exclusions {
            lines.push(format!("- {item}"));
        }
    }
    lines.push(String::new());
    lines.push("Report to judge:".to_string());
    lines.push(report_text.to_string());
    lines.push(String::new());
    lines.push(
        "Reply with exactly one of these. If the report meets every done-when item and stays out of scope, reply: VERDICT: PASS".to_string(),
    );
    lines.push(
        "If it misses a done-when item or does something out of scope, reply: VERDICT: FAIL followed by one line saying what was missed or violated.".to_string(),
    );
    lines.join("\n")
}

/// Read a judge reply. (passed, reason) or None when the reply
/// cannot be parsed. A pass carries no reason. A fail's reason is the
/// rest of the verdict line, or the next non-empty line when the
/// verdict line carries none.
pub fn parse_verdict(text: &str) -> Option<(bool, String)> {
    if text.is_empty() {
        return None;
    }
    let lines: Vec<String> = text.trim().lines().map(|l| l.trim().to_string()).collect();
    for (index, line) in lines.iter().enumerate() {
        if !line.to_uppercase().starts_with("VERDICT:") {
            continue;
        }
        let rest = line["VERDICT:".len()..].trim().to_string();
        let upper = rest.to_uppercase();
        if upper.starts_with("PASS") {
            return Some((true, String::new()));
        }
        if upper.starts_with("FAIL") {
            let mut reason = rest["FAIL".len()..]
                .trim_matches(|c| c == '-' || c == ':' || c == ' ')
                .trim()
                .to_string();
            if reason.is_empty() {
                let following: Vec<&String> = lines[index + 1..]
                    .iter()
                    .filter(|l| !l.is_empty())
                    .collect();
                reason = following.first().map(|s| s.to_string()).unwrap_or_default();
            }
            return Some((
                false,
                if reason.is_empty() {
                    "no reason given".to_string()
                } else {
                    reason
                },
            ));
        }
    }
    None
}

/// The report as text for the judge: summary plus per-agent outputs, capped.
pub fn report_text(summary: &str, results: &[(String, String)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let summary = summary.trim();
    if !summary.is_empty() {
        lines.push(summary.to_string());
    }
    for (agent_name, output) in results {
        let output = output.trim();
        if !output.is_empty() {
            lines.push(format!("[{agent_name}] {output}"));
        }
    }
    let mut text = lines.join("\n\n");
    if text.chars().count() > REPORT_TEXT_CHARS {
        let head: String = text.chars().take(REPORT_TEXT_CHARS).collect();
        text = format!("{head}... [truncated]");
    }
    text
}

/// The outcome of one judging call.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeOutcome {
    pub verdict: String,
    pub reason: String,
}

impl JudgeOutcome {
    pub fn to_json(&self) -> Value {
        json!({"verdict": self.verdict, "reason": self.reason})
    }
}

/// Ask the Reviewer to judge one report. Never raises.
///
/// Returns pass, fail, or skipped with a reason.
pub fn run_judge(registry: &AgentRegistry, contract: &Contract, text: &str) -> JudgeOutcome {
    let judge = match registry.get(JUDGE_AGENT_NAME) {
        Ok(agent) => agent,
        Err(_) => {
            return JudgeOutcome {
                verdict: "skipped".to_string(),
                reason: "no reviewer agent in the registry".to_string(),
            };
        }
    };
    let messages = vec![
        ChatMessage::new(
            "system",
            format!(
                "{}\n\nYou are judging this run, not doing the work.",
                judge.system_prompt
            ),
        ),
        ChatMessage::new("user", judge_prompt(contract, text)),
    ];
    let reply = match judge.client().chat(&judge.model, &messages) {
        Ok(reply) => reply,
        Err(e) => {
            return JudgeOutcome {
                verdict: "skipped".to_string(),
                reason: format!("judge call failed: {e}"),
            };
        }
    };
    match parse_verdict(&reply) {
        None => JudgeOutcome {
            verdict: "skipped".to_string(),
            reason: "judge reply could not be parsed".to_string(),
        },
        Some((passed, reason)) => JudgeOutcome {
            verdict: if passed {
                "pass".to_string()
            } else {
                "fail".to_string()
            },
            reason,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::Contract;

    fn contract() -> Contract {
        Contract {
            goal: "Write a short history of the abacus".to_string(),
            scope: vec![
                "origins in Mesopotamia".to_string(),
                "spread to China".to_string(),
            ],
            exclusions: vec!["modern calculators".to_string(), "buying links".to_string()],
            done_when: vec![
                "covers the origins".to_string(),
                "under 500 words".to_string(),
            ],
            checks: vec![],
        }
    }

    #[test]
    fn parse_verdict_variants() {
        assert_eq!(parse_verdict("VERDICT: PASS"), Some((true, String::new())));
        assert_eq!(parse_verdict("verdict: pass"), Some((true, String::new())));
        assert_eq!(
            parse_verdict("VERDICT: FAIL\nThe report never covers the origins."),
            Some((false, "The report never covers the origins.".to_string()))
        );
        assert_eq!(
            parse_verdict("VERDICT: FAIL"),
            Some((false, "no reason given".to_string()))
        );
        assert_eq!(parse_verdict("Looks fine to me."), None);
        assert_eq!(parse_verdict(""), None);
    }

    #[test]
    fn judge_prompt_names_terms() {
        let text = judge_prompt(&contract(), "The abacus began in Mesopotamia.");
        assert!(text.contains("Write a short history of the abacus"));
        assert!(text.contains("covers the origins"));
        assert!(text.contains("modern calculators"));
        assert!(text.contains("The abacus began in Mesopotamia."));
        assert!(text.contains("not the builder"));
    }
}
