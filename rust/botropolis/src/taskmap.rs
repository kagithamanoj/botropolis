//! A human-readable task map for one run.
//!
//! Run state JSON and the ledger are the machine record of a run. The
//! task map is the readable one: a short markdown file written next to
//! the run state at `botropolis/data/runs/<run_id>.map.md` (gitignored,
//! same as run state). It says, in words, where the run stands: the
//! goal and what counts as done, the recorded plan with each step
//! marked done or pending, the tool calls made so far with one-line
//! outcomes, and any blockers.
//!
//! The plan marks are a progress sketch, not a proof. Plan steps are
//! marked done in order as tool calls succeed: the first successful
//! call completes the first step, the next the second, and so on. A
//! plan step that needs no tool stays pending until the run finishes.
//! The map never claims a step is done because the model said so; it
//! counts what actually ran.
//!
//! A resumed run reads its map back and the loop attaches it to the
//! conversation, so the continuing agent sees the work in words, not
//! just raw steps.
//!
//! Writing a map never breaks a run: a failed write is swallowed, the
//! same as a failed run-state write. Mirrors
//! `botropolis/core/taskmap.py`.

use regex::Regex;
use serde_json::Value;
use std::path::PathBuf;

use crate::runstate::RunStore;

pub const MAP_SUFFIX: &str = ".map.md";

/// Split a recorded plan into its steps, in order.
///
/// The first line carries the PLAN: prefix; the steps are the lines
/// after it, with list markers and numbering stripped. Empty lines
/// drop out. A plan that is one line with no steps after it yields
/// that line as its single step.
pub fn plan_steps(plan_text: Option<&str>) -> Vec<String> {
    let Some(text) = plan_text else {
        return Vec::new();
    };
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let lines: Vec<String> = text.lines().map(|l| l.trim().to_string()).collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut first = lines[0].clone();
    if first.to_uppercase().starts_with("PLAN:") {
        first = first[5..].trim().to_string();
    }
    let mut rest = Vec::new();
    if !first.is_empty() {
        rest.push(first);
    }
    rest.extend(lines[1..].iter().cloned());
    let marker_re = Regex::new(r"^(\d+[.)]|[-*])\s+").unwrap();
    rest.into_iter()
        .map(|line| marker_re.replace(&line, "").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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

fn steps_of(state: &Value) -> Vec<Value> {
    state
        .get("steps")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Render a run state dict as a task map in markdown.
pub fn render_map(state: &Value) -> String {
    let run_id = state.get("run_id").and_then(|v| v.as_str()).unwrap_or("");
    let task = state.get("task").cloned().unwrap_or(Value::Null);
    let contract = state
        .get("contract")
        .cloned()
        .or_else(|| task.get("contract").cloned())
        .unwrap_or(Value::Null);
    let steps = steps_of(state);
    let status = state
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("running");
    let mut lines = vec![format!("# Task map: {run_id}"), String::new()];
    lines.push(format!(
        "Agent: {}",
        state.get("agent").and_then(|v| v.as_str()).unwrap_or("")
    ));
    lines.push(format!(
        "Task: {}",
        task.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
    ));
    lines.push(format!("Status: {status}"));
    if let Some(reason) = state.get("reason").and_then(|v| v.as_str()) {
        if !reason.is_empty() {
            lines.push(format!("Reason: {reason}"));
        }
    }
    lines.push(String::new());
    if let Some(goal) = contract.get("goal").and_then(|v| v.as_str()) {
        if !goal.is_empty() {
            lines.push("## Goal".to_string());
            lines.push(goal.to_string());
            lines.push(String::new());
        }
    }
    if let Some(done_when) = contract.get("done_when").and_then(|v| v.as_array()) {
        if !done_when.is_empty() {
            lines.push("## Done when".to_string());
            for item in done_when {
                lines.push(format!("- {}", item.as_str().unwrap_or("")));
            }
            lines.push(String::new());
        }
    }
    let plan = plan_steps(state.get("plan").and_then(|v| v.as_str()));
    if !plan.is_empty() {
        let done_count = steps
            .iter()
            .filter(|s| s.get("success").and_then(|v| v.as_bool()).unwrap_or(true))
            .count()
            .min(plan.len());
        lines.push("## Plan".to_string());
        for (index, step) in plan.iter().enumerate() {
            let mark = if index < done_count { "x" } else { " " };
            lines.push(format!("- [{mark}] {step}"));
        }
        lines.push(String::new());
    }
    lines.push("## Tool calls".to_string());
    if steps.is_empty() {
        lines.push("- none yet".to_string());
    }
    for step in &steps {
        let outcome = if step
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
        {
            "ok"
        } else {
            "FAILED"
        };
        let tool = step.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let args = step.get("args").map(|v| v.to_string()).unwrap_or_default();
        let result = step
            .get("result_preview")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        lines.push(format!(
            "- {tool} {} -> {outcome}: {}",
            one_line(&args, 120),
            one_line(result, 120)
        ));
    }
    lines.push(String::new());
    let blockers: Vec<&Value> = steps
        .iter()
        .filter(|s| !s.get("success").and_then(|v| v.as_bool()).unwrap_or(true))
        .collect();
    if !blockers.is_empty() || status == "escalated" {
        lines.push("## Blockers".to_string());
        for step in &blockers {
            let tool = step.get("tool").and_then(|v| v.as_str()).unwrap_or("");
            let result = step
                .get("result_preview")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            lines.push(format!("- {tool} failed: {}", one_line(result, 120)));
        }
        if status == "escalated" {
            if let Some(reason) = state.get("reason").and_then(|v| v.as_str()) {
                lines.push(format!("- escalated: {reason}"));
            }
        }
        lines.push(String::new());
    }
    if let Some(answer) = state.get("answer").and_then(|v| v.as_str()) {
        if !answer.is_empty() {
            lines.push("## Answer".to_string());
            lines.push(answer.trim().to_string());
            lines.push(String::new());
        }
    }
    lines.join("\n").trim().to_string() + "\n"
}

/// Where one run's map file lives, next to its state file.
pub fn map_path(store: &RunStore, run_id: &str) -> PathBuf {
    store.root.join(format!("{run_id}{MAP_SUFFIX}"))
}

/// Write one run's map from its state dict. Never raises.
pub fn write_map(store: &RunStore, state: &Value) {
    let run_id = match state.get("run_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return,
    };
    if std::fs::create_dir_all(&store.root).is_err() {
        return;
    }
    let _ = std::fs::write(map_path(store, &run_id), render_map(state));
}

/// Read one run's map. Falls back to rendering from state when the map
/// file is missing (older runs predate maps). Returns an error when
/// neither exists.
pub fn read_map(store: &RunStore, run_id: &str) -> Result<String, String> {
    let path = map_path(store, run_id);
    if path.is_file() {
        return std::fs::read_to_string(&path).map_err(|e| e.to_string());
    }
    let state = store.load(run_id)?;
    Ok(render_map(&state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_steps_strips_markers() {
        let plan = "PLAN: Work out 6 times 7.\nThen work out 8 times 9.\nThen answer.";
        assert_eq!(
            plan_steps(Some(plan)),
            vec![
                "Work out 6 times 7.",
                "Then work out 8 times 9.",
                "Then answer."
            ]
        );
        assert_eq!(
            plan_steps(Some("PLAN: one step only")),
            vec!["one step only"]
        );
        assert_eq!(
            plan_steps(Some("PLAN: First.\n1. Second\n- Third")),
            vec!["First.", "Second", "Third"]
        );
        assert!(plan_steps(None).is_empty());
        assert!(plan_steps(Some("")).is_empty());
    }

    #[test]
    fn render_map_marks_plan_progress_and_blockers() {
        let state = json!({
            "run_id": "run1",
            "agent": "TestAgent",
            "task": {"description": "Work out some sums"},
            "contract": {"goal": "Finish the sums", "done_when": ["both sums done"]},
            "steps": [
                {"tool": "calculator", "args": {"expression": "6 * 7"}, "success": true, "result_preview": "42", "elapsed_ms": 1.0},
                {"tool": "calculator", "args": {"expression": "1 / 0"}, "success": false, "result_preview": "division by zero", "elapsed_ms": 1.0},
            ],
            "plan": "PLAN: Work out 6 times 7.\nThen work out 1 over 0.",
            "status": "finished",
            "answer": "42, and 1 over 0 has no answer.",
        });
        let map = render_map(&state);
        assert!(map.contains("# Task map: run1"));
        assert!(map.contains("## Goal"));
        assert!(map.contains("Finish the sums"));
        assert!(map.contains("- [x] Work out 6 times 7."));
        assert!(map.contains("- [ ] Then work out 1 over 0."));
        assert!(map.contains("calculator"));
        assert!(map.contains("FAILED"));
        assert!(map.contains("## Blockers"));
        assert!(map.contains("## Answer"));
    }
}
