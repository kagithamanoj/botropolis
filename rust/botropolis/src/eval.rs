//! Scenario evals for Botropolis agents.
//!
//! Each scenario is a YAML file describing a scripted model
//! conversation: the replies the model would give, and what the agent
//! should do with them. The runner plays the replies through a real
//! Agent and checks the outcome, so tool-use behavior gets regression
//! coverage without live model calls.
//!
//! Scenario shape:
//!
//! ```yaml
//! name: calculator_two_step
//! description: Agent multiplies with the calculator, then answers.
//! toolkit: [calculator]
//! task: What is 6 times 7?
//! replies:
//!   - 'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}'
//!   - "FINAL: 42"
//! expect:
//!   output_contains: "42"
//!   tool_calls:
//!     - tool: calculator
//!       args: {expression: "6 * 7"}
//! ```
//!
//! Scenarios that need files or a stubbed data source can also set:
//!
//! ```yaml
//! env: {BOTROPOLIS_WORKSPACE: "{tmp}/workspace"}
//! setup_files: {workspace/resume.txt: "resume contents"}
//! expect:
//!   file_contains:
//!     - path: "{tmp}/applications.jsonl"
//!       contains: "Acme Corp"
//! ```
//!
//! A scenario can also stand up other agents for the consult tool to
//! ask, with `consult_agents` (a list of spec dicts). They are written
//! to a scratch agents directory and registered for the run; they share
//! the scenario's scripted client, so their replies come from the same
//! `replies` queue, in call order.
//!
//! A scenario can flag tools that need human approval with
//! `needs_approval` (a list of tool names). Those calls are blocked at
//! the approval wall and never run; the scenario checks the blocked
//! call like any other, with `success: false`.
//!
//! `env` values and file paths may use {tmp} (a fresh temp directory
//! for the run) and {scenarios} (the bundled scenarios directory).
//! Environment variables are restored when the run ends.
//!
//! Run all scenarios with: botropolis evals
//! Mirrors `botropolis/eval/runner.py`.

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::Agent;
use crate::models::ScriptedClient;
use crate::registry::AgentRegistry;
use crate::schemas::Task;
use crate::tools::consult::set_consult_registry;

/// The bundled scenarios directory.
pub fn scenarios_dir() -> PathBuf {
    crate::paths::repo_root()
        .join("botropolis")
        .join("eval")
        .join("scenarios")
}

/// The outcome of one scenario run.
#[derive(Debug, Clone)]
pub struct EvalResult {
    pub name: String,
    pub passed: bool,
    pub details: Vec<String>,
}

/// Load every *.yaml scenario from a directory, sorted by name.
pub fn load_scenarios(directory: &Path) -> Vec<Value> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut scenarios = Vec::new();
    for path in paths {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_yaml::from_str::<Value>(&text) {
                scenarios.push(value);
            }
        }
    }
    scenarios
}

fn expand(text: &str, tmp_dir: &Path) -> String {
    text.replace("{tmp}", &tmp_dir.to_string_lossy())
        .replace("{scenarios}", &scenarios_dir().to_string_lossy())
}

fn check_tool_calls(expected: &[Value], actual: &[crate::schemas::ToolCall]) -> Vec<String> {
    let mut problems = Vec::new();
    if actual.len() != expected.len() {
        problems.push(format!(
            "expected {} tool calls, got {}",
            expected.len(),
            actual.len()
        ));
        return problems;
    }
    for (index, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        if got.tool != want.get("tool").and_then(|v| v.as_str()).unwrap_or("") {
            problems.push(format!(
                "call {index}: expected tool {:?}, got {:?}",
                want.get("tool"),
                got.tool
            ));
        }
        let want_args = want.get("args").cloned().unwrap_or(Value::Null);
        let got_args = serde_json::to_value(&got.args).unwrap_or(Value::Null);
        if got_args != want_args {
            problems.push(format!(
                "call {index}: expected args {want_args}, got {got_args}"
            ));
        }
        if let Some(want_success) = want.get("success").and_then(|v| v.as_bool()) {
            if got.success != want_success {
                problems.push(format!(
                    "call {index}: expected success={want_success}, got {}",
                    got.success
                ));
            }
        }
    }
    problems
}

fn fresh_tmp_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join("botropolis-eval").join(format!(
        "{}-{}",
        std::process::id(),
        nanos
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Run one scenario dict. Never raises; failures become details.
pub fn run_scenario(scenario: &Value) -> EvalResult {
    let name = scenario
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unnamed>")
        .to_string();
    let mut details: Vec<String> = Vec::new();
    let tmp_dir = fresh_tmp_dir();
    let mut saved_env: HashMap<String, Option<String>> = HashMap::new();
    let consult_agents = scenario
        .get("consult_agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let outcome = run_scenario_inner(scenario, &tmp_dir, &mut saved_env, &consult_agents);
    // Restore the environment and clear the consult pin, whatever happened.
    if !consult_agents.is_empty() {
        set_consult_registry(None);
    }
    for (key, value) in saved_env {
        match value {
            Some(value) => std::env::set_var(&key, value),
            None => std::env::remove_var(&key),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
    match outcome {
        Ok(found) => details.extend(found),
        Err(error) => details.push(format!("error: {error}")),
    }
    EvalResult {
        name,
        passed: details.is_empty(),
        details,
    }
}

fn run_scenario_inner(
    scenario: &Value,
    tmp_dir: &Path,
    saved_env: &mut HashMap<String, Option<String>>,
    consult_agents: &[Value],
) -> Result<Vec<String>, String> {
    for (key, value) in scenario
        .get("env")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
    {
        saved_env.insert(key.clone(), std::env::var(&key).ok());
        let expanded = expand(value.as_str().unwrap_or(""), tmp_dir);
        std::env::set_var(&key, expanded);
    }
    for (rel, content) in scenario
        .get("setup_files")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
    {
        let dest = tmp_dir.join(expand(&rel, tmp_dir));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, content.as_str().unwrap_or("")).map_err(|e| e.to_string())?;
    }
    let replies: Vec<String> = scenario
        .get("replies")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    let client = Arc::new(ScriptedClient::new(replies));
    if !consult_agents.is_empty() {
        let agents_dir = tmp_dir.join("agents");
        for spec in consult_agents {
            let dept = spec
                .get("department")
                .and_then(|v| v.as_str())
                .unwrap_or("research");
            let dept_dir = agents_dir.join(dept);
            std::fs::create_dir_all(&dept_dir).map_err(|e| e.to_string())?;
            let spec_name = spec.get("name").and_then(|v| v.as_str()).unwrap_or("agent");
            let spec_path = dept_dir.join(format!("{spec_name}.yaml"));
            let text = serde_yaml::to_string(spec).map_err(|e| e.to_string())?;
            std::fs::write(&spec_path, text).map_err(|e| e.to_string())?;
        }
        let registry = AgentRegistry::load_from(
            &agents_dir,
            Some(client.clone() as Arc<dyn crate::models::ModelClient>),
        )
        .map_err(|e| e.to_string())?;
        set_consult_registry(Some(Arc::new(registry)));
    }
    let spec = serde_json::json!({
        "name": "EvalAgent",
        "title": "Eval agent",
        "department": "research",
        "specialty": "scenario evals",
        "model": "stub",
        "system_prompt": "You are an eval agent.",
        "toolkit": scenario.get("toolkit").cloned().unwrap_or(Value::Null),
    });
    let spec_path = tmp_dir.join("evalagent.yaml");
    std::fs::write(
        &spec_path,
        serde_yaml::to_string(&spec).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let agent = Agent::load(
        &spec_path,
        Some(client.clone() as Arc<dyn crate::models::ModelClient>),
    )
    .map_err(|e| e.to_string())?;
    let mut task = Task::new(scenario.get("task").and_then(|v| v.as_str()).unwrap_or(""));
    task.department = Some("research".to_string());
    task.agent_name = Some("EvalAgent".to_string());
    task.needs_approval = scenario
        .get("needs_approval")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let result = agent.run(task, Default::default());

    let mut details: Vec<String> = Vec::new();
    let expect = scenario.get("expect").cloned().unwrap_or(Value::Null);
    if let Some(needle) = expect.get("output_contains").and_then(|v| v.as_str()) {
        if !result.output.contains(needle) {
            details.push(format!("output does not contain {needle:?}"));
        }
    }
    if let Some(want) = expect.get("output_equals").and_then(|v| v.as_str()) {
        if result.output.trim() != want.trim() {
            details.push(format!("output {:?} != {want:?}", result.output));
        }
    }
    if let Some(count) = expect.get("tool_call_count").and_then(|v| v.as_u64()) {
        if result.tool_calls.len() as u64 != count {
            details.push(format!(
                "expected {count} tool calls, got {}",
                result.tool_calls.len()
            ));
        }
    }
    if let Some(expected_calls) = expect.get("tool_calls").and_then(|v| v.as_array()) {
        details.extend(check_tool_calls(expected_calls, &result.tool_calls));
    }
    if let Some(want_success) = expect.get("success").and_then(|v| v.as_bool()) {
        if result.success != want_success {
            details.push(format!(
                "expected success={want_success}, got {}",
                result.success
            ));
        }
    }
    for check in expect
        .get("file_contains")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let path = PathBuf::from(expand(
            check.get("path").and_then(|v| v.as_str()).unwrap_or(""),
            tmp_dir,
        ));
        let needle = check
            .get("contains")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match std::fs::read_to_string(&path) {
            Err(_) => details.push(format!(
                "expected file {:?} to exist after the run",
                check.get("path")
            )),
            Ok(content) => {
                if !content.contains(&needle) {
                    details.push(format!(
                        "file {:?} does not contain {needle:?}",
                        check.get("path")
                    ));
                }
            }
        }
    }
    for check in expect
        .get("file_absent")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let wanted = check.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let path = PathBuf::from(expand(wanted, tmp_dir));
        if path.exists() {
            details.push(format!(
                "expected file {wanted:?} to be absent after the run"
            ));
        }
    }
    Ok(details)
}

/// Run every scenario in a directory.
pub fn run_all(directory: &Path) -> Vec<EvalResult> {
    load_scenarios(directory).iter().map(run_scenario).collect()
}

/// Run the evals and print the results, returning an exit code.
pub fn run_evals(directory: Option<&Path>) -> i32 {
    let dir = match directory {
        Some(dir) => dir.to_path_buf(),
        None => scenarios_dir(),
    };
    let results = run_all(&dir);
    let passed = results.iter().filter(|r| r.passed).count();
    for result in &results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        println!("[{status}] {}", result.name);
        for detail in &result.details {
            println!("       - {detail}");
        }
    }
    println!("\n{passed}/{} scenarios passed", results.len());
    if passed == results.len() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passing_scenario() {
        let scenario = serde_json::json!({
            "name": "passing",
            "toolkit": ["calculator"],
            "task": "What is 6 times 7?",
            "replies": [
                "ACTION: {\"tool\": \"calculator\", \"args\": {\"expression\": \"6 * 7\"}}",
                "FINAL: {\"summary\": \"42\", \"details\": \"\", \"key_points\": []}",
            ],
            "expect": {"output_contains": "42", "tool_call_count": 1},
        });
        let result = run_scenario(&scenario);
        assert!(result.passed, "details: {:?}", result.details);
    }

    #[test]
    fn failing_output_check_reports_detail() {
        let scenario = serde_json::json!({
            "name": "failing",
            "toolkit": ["calculator"],
            "task": "What is 6 times 7?",
            "replies": [
                "ACTION: {\"tool\": \"calculator\", \"args\": {\"expression\": \"6 * 7\"}}",
                "FINAL: {\"summary\": \"42\", \"details\": \"\", \"key_points\": []}",
            ],
            "expect": {"output_contains": "43"},
        });
        let result = run_scenario(&scenario);
        assert!(!result.passed);
        assert!(result.details.iter().any(|d| d.contains("43")));
    }

    #[test]
    fn unknown_toolkit_does_not_crash_runner() {
        let scenario = serde_json::json!({
            "name": "unknown-toolkit",
            "toolkit": ["no_such_tool"],
            "task": "Do a thing.",
            "replies": ["FINAL: {\"summary\": \"done\", \"details\": \"\", \"key_points\": []}"],
            "expect": {"success": true},
        });
        let result = run_scenario(&scenario);
        assert!(
            !result.passed,
            "an unknown tool fails the scenario, not the runner"
        );
        assert!(result.details.iter().any(|d| d.contains("error")));
    }

    #[test]
    fn needs_approval_blocks_tool_and_writes_nothing() {
        let scenario = serde_json::json!({
            "name": "approval-blocked",
            "toolkit": ["job_apply"],
            "needs_approval": ["job_apply"],
            "env": {
                "BOTROPOLIS_WORKSPACE": "{tmp}/workspace",
                "BOTROPOLIS_APPLICATIONS_LOG": "{tmp}/applications.jsonl",
            },
            "setup_files": {"workspace/resume.txt": "Eval resume."},
            "task": "Prepare an application.",
            "replies": [
                "ACTION: {\"tool\": \"job_apply\", \"args\": {\"title\": \"Dev\", \"company\": \"Acme Corp\", \"url\": \"https://example.com/jobs/1\", \"resume\": \"resume.txt\"}}",
                "FINAL: {\"summary\": \"blocked: job_apply needs human approval\", \"details\": \"\", \"key_points\": []}",
            ],
            "expect": {
                "output_contains": "needs human approval",
                "tool_calls": [
                    {"tool": "job_apply", "args": {"title": "Dev", "company": "Acme Corp", "url": "https://example.com/jobs/1", "resume": "resume.txt"}, "success": false}
                ],
                "file_absent": [{"path": "{tmp}/applications.jsonl"}],
            },
        });
        let result = run_scenario(&scenario);
        assert!(result.passed, "details: {:?}", result.details);
    }

    #[test]
    fn file_absent_fails_when_file_exists() {
        let scenario = serde_json::json!({
            "name": "absent-check",
            "toolkit": ["calculator"],
            "task": "Do math.",
            "setup_files": {"written.txt": "already here"},
            "replies": [
                "FINAL: {\"summary\": \"done\", \"details\": \"\", \"key_points\": []}",
            ],
            "expect": {
                "output_contains": "done",
                "file_absent": [{"path": "{tmp}/written.txt"}],
            },
        });
        let result = run_scenario(&scenario);
        assert!(!result.passed);
        assert!(result.details.iter().any(|d| d.contains("absent")));
    }

    #[test]
    fn run_all_loads_bundled_scenarios() {
        let scenarios = load_scenarios(&scenarios_dir());
        assert_eq!(scenarios.len(), 15);
        let names: Vec<&str> = scenarios
            .iter()
            .filter_map(|s| s.get("name").and_then(|v| v.as_str()))
            .collect();
        for want in [
            "plan_first_then_act",
            "check_revises_answer",
            "same_error_escalates",
            "malformed_action_recovers",
            "approval_wall_blocks_apply",
        ] {
            assert!(names.contains(&want), "missing scenario {want}");
        }
    }
}
