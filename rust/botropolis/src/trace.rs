//! Trace export: read a run back as a trace.
//!
//! The ledger and run state already record every step, but there was
//! no way to read a run back as one story. `build_trace()` assembles
//! it: the task, the plan, the tool calls with one-line outcomes, the
//! ledger events for the run, and the final status and answer. The CLI
//! prints it as plain readable text, or as JSON with --json.
//!
//! The run state is the source of truth: a trace needs it, and a
//! missing run is an error. Ledger events are included when a
//! persisted ledger exists (`BOTROPOLIS_LEDGER_DB`, or
//! `botropolis/data/ledger.db` when present); without one, the trace
//! carries an empty events list instead of failing. Reading a trace
//! never creates files: a ledger path that does not exist is simply
//! skipped. Mirrors `botropolis/core/trace.py`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};

use crate::ledger::{Ledger, LedgerEvent};
use crate::runstate::RunStore;
use crate::taskmap::plan_steps;

pub const DEFAULT_LEDGER_NAME: &str = "ledger.db";

/// Where a persisted ledger would live, if one exists.
///
/// `BOTROPOLIS_LEDGER_DB` wins; otherwise the default file under
/// `botropolis/data`. None when no ledger file exists, so callers
/// never create one by accident.
pub fn ledger_db_path() -> Option<PathBuf> {
    if let Ok(override_path) = env::var("BOTROPOLIS_LEDGER_DB") {
        if !override_path.is_empty() {
            let path = PathBuf::from(override_path);
            return if path.is_file() { Some(path) } else { None };
        }
    }
    let default = crate::paths::data_dir().join(DEFAULT_LEDGER_NAME);
    if default.is_file() {
        Some(default)
    } else {
        None
    }
}

fn ledger_events(run_id: &str, ledger_path: Option<&Path>) -> Vec<LedgerEvent> {
    let Some(path) = ledger_path else {
        return Vec::new();
    };
    if !path.is_file() {
        return Vec::new();
    }
    match Ledger::open(path) {
        Ok(ledger) => ledger.events(run_id).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// One tool step in a trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub tool: String,
    pub args: Value,
    pub success: bool,
    pub outcome: String,
}

/// One run read back as a story.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub run_id: String,
    pub agent: String,
    pub task: String,
    pub status: String,
    pub reason: String,
    pub plan: Vec<String>,
    pub steps: Vec<TraceStep>,
    pub events: Vec<LedgerEvent>,
    pub answer: String,
}

/// Assemble one run's trace. Returns an error when the run does not
/// exist. `ledger_path`, when given, points at a persisted ledger DB;
/// otherwise the default is looked up.
pub fn build_trace(
    store: &RunStore,
    run_id: &str,
    ledger_path: Option<&Path>,
) -> Result<Trace, String> {
    let state = store.load(run_id)?;
    let ledger_path = match ledger_path {
        Some(p) => Some(p.to_path_buf()),
        None => ledger_db_path(),
    };
    let task = state.get("task").cloned().unwrap_or(Value::Null);
    let steps = state
        .get("steps")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|step| TraceStep {
            tool: step
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            args: step.get("args").cloned().unwrap_or(Value::Null),
            success: step
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            outcome: step
                .get("result_preview")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    Ok(Trace {
        run_id: state
            .get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or(run_id)
            .to_string(),
        agent: state
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        task: task
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: state
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        reason: state
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        plan: plan_steps(state.get("plan").and_then(|v| v.as_str())),
        steps,
        events: ledger_events(run_id, ledger_path.as_deref()),
        answer: state
            .get("answer")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn one_line(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > limit {
        let head: String = flat.chars().take(limit).collect();
        format!("{head}...")
    } else if flat.is_empty() {
        "(empty result)".to_string()
    } else {
        flat
    }
}

/// Render a trace as plain readable text.
pub fn render_text(trace: &Trace) -> String {
    let mut lines = vec![format!("Trace: {}", trace.run_id), String::new()];
    lines.push(format!("Agent: {}", trace.agent));
    lines.push(format!("Task: {}", trace.task));
    lines.push(format!("Status: {}", trace.status));
    if !trace.reason.is_empty() {
        lines.push(format!("Reason: {}", trace.reason));
    }
    lines.push(String::new());
    if !trace.plan.is_empty() {
        lines.push("Plan:".to_string());
        for (index, step) in trace.plan.iter().enumerate() {
            lines.push(format!("  {}. {step}", index + 1));
        }
        lines.push(String::new());
    }
    lines.push("Steps:".to_string());
    if trace.steps.is_empty() {
        lines.push("  none yet".to_string());
    }
    for (index, step) in trace.steps.iter().enumerate() {
        let outcome = if step.success { "ok" } else { "FAILED" };
        lines.push(format!(
            "  {}. {} {} -> {outcome}: {}",
            index + 1,
            step.tool,
            one_line(&step.args.to_string(), 120),
            one_line(&step.outcome, 120)
        ));
    }
    lines.push(String::new());
    if !trace.events.is_empty() {
        lines.push("Events:".to_string());
        for event in &trace.events {
            let payload = serde_json::to_string(&event.payload).unwrap_or_default();
            lines.push(format!(
                "  {}. [{}] {}: {}",
                event.seq,
                event.actor,
                event.kind,
                one_line(&payload, 120)
            ));
        }
        lines.push(String::new());
    }
    if !trace.answer.is_empty() {
        lines.push("Answer:".to_string());
        lines.push(trace.answer.trim().to_string());
        lines.push(String::new());
    }
    lines.join("\n").trim().to_string() + "\n"
}

/// A trace as a JSON value, for --json output.
pub fn trace_to_json(trace: &Trace) -> Value {
    json!(trace)
}
