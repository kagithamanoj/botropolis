//! Tool loop tests: plan and verify, loop exits, and the approval wall.
//! Mirrors the Python tests in test_loop_plan_check.py,
//! test_loop_exits.py, and test_approval.py.

use botropolis::agent::{Agent, RunOpts};
use botropolis::models::{ChatMessage, ModelClient, ModelError, Usage};
use botropolis::schemas::Task;
use botropolis::tool_loop::{parse_check_reply, parse_plan_reply, LoopStateSnapshot};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CALC_ACTION: &str = r#"ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}"#;
const BAD_EXPR: &str = r#"ACTION: {"tool": "calculator", "args": {"expression": "2 +"}}"#;
const OTHER_BAD_EXPR: &str = r#"ACTION: {"tool": "calculator", "args": {"expression": "foo("}}"#;
const GOOD_EXPR: &str = r#"ACTION: {"tool": "calculator", "args": {"expression": "1 + 1"}}"#;
const PLAN: &str = "PLAN: First work out 6 times 7 with the calculator, then answer.";

/// Scripted stand-in for ModelClient. Replies play in order, every
/// conversation is kept, and usage per call is configurable. Reports
/// the stub provider, like the Python test clients.
struct FakeClient {
    replies: Mutex<Vec<String>>,
    calls: Mutex<usize>,
    seen: Mutex<Vec<Vec<ChatMessage>>>,
    usage_per_call: Usage,
    delay: Duration,
}

impl FakeClient {
    fn new(replies: Vec<String>) -> Self {
        FakeClient {
            replies: Mutex::new(replies),
            calls: Mutex::new(0),
            seen: Mutex::new(Vec::new()),
            usage_per_call: Usage::default(),
            delay: Duration::from_secs(0),
        }
    }

    fn with_usage(replies: Vec<String>, usage_per_call: Usage) -> Self {
        let mut client = FakeClient::new(replies);
        client.usage_per_call = usage_per_call;
        client
    }

    fn with_delay(replies: Vec<String>, delay: Duration) -> Self {
        let mut client = FakeClient::new(replies);
        client.delay = delay;
        client
    }

    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }

    fn seen(&self) -> Vec<Vec<ChatMessage>> {
        self.seen.lock().unwrap().clone()
    }
}

impl ModelClient for FakeClient {
    fn chat(&self, _model_id: &str, messages: &[ChatMessage]) -> Result<String, ModelError> {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        let index = *calls;
        self.seen.lock().unwrap().push(messages.to_vec());
        let replies = self.replies.lock().unwrap();
        if replies.is_empty() {
            return Ok(String::new());
        }
        if index <= replies.len() {
            Ok(replies[index - 1].clone())
        } else {
            Ok(replies[replies.len() - 1].clone())
        }
    }

    fn provider_for(&self, _model_id: &str) -> String {
        "stub".to_string()
    }

    fn last_usage(&self) -> Usage {
        self.usage_per_call
    }
}

fn make_agent(dir: &std::path::Path, client: Arc<FakeClient>, spec_extra: &str) -> Agent {
    let path = dir.join("testagent.yaml");
    std::fs::write(
        &path,
        format!(
            "name: TestAgent\ntitle: Tester\ndepartment: research\nspecialty: testing\nmodel: stub\nsystem_prompt: You are a test agent.\ntoolkit: [calculator]\n{spec_extra}"
        ),
    )
    .unwrap();
    Agent::load(&path, Some(client)).unwrap()
}

fn run_with_client(
    dir: &std::path::Path,
    replies: Vec<&str>,
    task: Task,
    opts: RunOpts,
) -> (Agent, Arc<FakeClient>, botropolis::schemas::AgentResult) {
    let client = Arc::new(FakeClient::new(
        replies.into_iter().map(|s| s.to_string()).collect(),
    ));
    let agent = make_agent(dir, Arc::clone(&client), "");
    let result = agent.run(task, opts);
    (agent, client, result)
}

#[test]
fn parse_plan_reply_variants() {
    assert!(parse_plan_reply(PLAN).is_some());
    assert!(parse_plan_reply("  \nPLAN: steps follow").is_some());
    assert!(parse_plan_reply("Here is my PLAN: not at the start").is_none());
    assert!(parse_plan_reply("FINAL: done").is_none());
    assert!(parse_plan_reply("").is_none());
}

#[test]
fn parse_check_reply_variants() {
    assert_eq!(
        parse_check_reply("CHECK: PASS", "draft"),
        ("draft".to_string(), false)
    );
    assert_eq!(
        parse_check_reply("FINAL: fixed", "draft"),
        ("fixed".to_string(), true)
    );
    assert_eq!(
        parse_check_reply("CHECK: REVISED\nFINAL: fixed", "draft"),
        ("fixed".to_string(), true)
    );
    assert_eq!(
        parse_check_reply("FINAL: draft", "draft"),
        ("draft".to_string(), false)
    );
    assert_eq!(
        parse_check_reply("looks fine to me", "draft"),
        ("draft".to_string(), false)
    );
    assert_eq!(parse_check_reply("", "draft"), ("draft".to_string(), false));
}

#[test]
fn plan_is_recorded_and_the_run_continues() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        PLAN.to_string(),
        CALC_ACTION.to_string(),
        "FINAL: 42".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let states: Arc<Mutex<Vec<LoopStateSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let states_clone = Arc::clone(&states);
    let result = agent.run(
        Task::new("what is 6 times 7"),
        RunOpts {
            run_id: Some("run-plan".to_string()),
            state_hook: Some(Box::new(move |snap| {
                states_clone.lock().unwrap().push(snap.clone());
            })),
            ..Default::default()
        },
    );
    assert!(result.success);
    assert_eq!(result.output, "42");
    assert_eq!(
        result
            .tool_calls
            .iter()
            .map(|c| c.tool.as_str())
            .collect::<Vec<_>>(),
        vec!["calculator"]
    );
    let states = states.lock().unwrap();
    let planned = states
        .iter()
        .find(|s| s.plan.as_deref() == Some(PLAN))
        .expect("plan recorded in a state snapshot");
    assert_eq!(planned.steps.len(), 0);
    let finished = states.iter().find(|s| s.status == "finished").unwrap();
    assert_eq!(finished.steps.len(), 1);
    // The call after the plan was told to begin.
    let seen = client.seen();
    assert!(seen.len() >= 2);
    assert!(seen[1]
        .iter()
        .any(|m| m.content.contains("Plan recorded. Begin")));
}

#[test]
fn plan_lands_in_events() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        PLAN.to_string(),
        "FINAL: done".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let kinds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let kinds_clone = Arc::clone(&kinds);
    let result = agent.run(
        Task::new("plan a small thing"),
        RunOpts {
            on_event: Some(Box::new(move |kind, _| {
                kinds_clone.lock().unwrap().push(kind.to_string());
            })),
            ..Default::default()
        },
    );
    assert!(result.success);
    let kinds = kinds.lock().unwrap();
    assert!(kinds.contains(&"plan_recorded".to_string()));
    assert!(kinds.contains(&"answer_checked".to_string()));
}

#[test]
fn run_without_a_plan_works_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![CALC_ACTION, "FINAL: 42", "CHECK: PASS"],
        Task::new("what is 6 times 7"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "42");
    assert_eq!(result.tool_calls.len(), 1);
}

#[test]
fn check_revision_is_accepted_once() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![CALC_ACTION, "FINAL: 41", "FINAL: 42"],
        Task::new("what is 6 times 7"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "42");
}

#[test]
fn unparseable_check_counts_as_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![CALC_ACTION, "FINAL: 42", "looks fine to me"],
        Task::new("what is 6 times 7"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "42");
}

#[test]
fn same_error_twice_stops_the_loop() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        BAD_EXPR.to_string(),
        BAD_EXPR.to_string(),
        GOOD_EXPR.to_string(),
        "FINAL: never reached".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let result = agent.run(Task::new("do math"), RunOpts::default());
    assert!(!result.success);
    assert_eq!(result.metadata.get("escalated"), Some(&json!(true)));
    let reason = result
        .metadata
        .get("escalation_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(reason.contains("calculator"));
    assert!(reason.contains("twice in a row"));
    assert_eq!(result.tool_calls.len(), 2);
    assert_eq!(client.calls(), 2);
}

#[test]
fn different_errors_do_not_escalate() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![BAD_EXPR, OTHER_BAD_EXPR, "FINAL: recovered"],
        Task::new("do math"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "recovered");
    assert_eq!(result.tool_calls.len(), 2);
    assert!(!result.metadata.contains_key("escalated"));
}

#[test]
fn same_error_not_in_a_row_does_not_escalate() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![BAD_EXPR, GOOD_EXPR, BAD_EXPR, "FINAL: fine"],
        Task::new("do math"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "fine");
    assert_eq!(result.tool_calls.len(), 3);
}

#[test]
fn token_budget_from_spec_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::with_usage(
        vec![
            GOOD_EXPR.to_string(),
            GOOD_EXPR.to_string(),
            GOOD_EXPR.to_string(),
            "FINAL: never reached".to_string(),
        ],
        Usage {
            input: 600,
            output: 0,
        },
    ));
    let agent = make_agent(dir.path(), Arc::clone(&client), "token_budget: 1000\n");
    let result = agent.run(Task::new("do math"), RunOpts::default());
    assert!(!result.success);
    let reason = result
        .metadata
        .get("escalation_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(reason.contains("token budget"));
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(client.calls(), 2);
}

#[test]
fn token_budget_as_run_param_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::with_usage(
        vec![GOOD_EXPR.to_string(), "FINAL: never reached".to_string()],
        Usage {
            input: 600,
            output: 0,
        },
    ));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let result = agent.run(
        Task::new("do math"),
        RunOpts {
            token_budget: Some(100),
            ..Default::default()
        },
    );
    assert!(!result.success);
    assert_eq!(result.tool_calls.len(), 0);
}

#[test]
fn time_budget_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::with_delay(
        vec![GOOD_EXPR.to_string(), "FINAL: never reached".to_string()],
        Duration::from_millis(300),
    ));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let result = agent.run(
        Task::new("do math"),
        RunOpts {
            time_budget_seconds: Some(0.1),
            ..Default::default()
        },
    );
    assert!(!result.success);
    let reason = result
        .metadata
        .get("escalation_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(reason.contains("time budget"));
}

#[test]
fn max_steps_runs_out() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![GOOD_EXPR, GOOD_EXPR, GOOD_EXPR],
        Task::new("do math"),
        RunOpts::default(),
    );
    // Default max_steps is 8 and the scripted client repeats its last
    // reply, so the run keeps acting; it ends with the last reply as
    // the answer and all 8 steps used.
    assert_eq!(result.tool_calls.len(), 8);
}

#[test]
fn flagged_tool_is_blocked_without_approval() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        CALC_ACTION.to_string(),
        "FINAL: could not compute".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let mut task = Task::new("what is 6 times 7");
    task.needs_approval = vec!["calculator".to_string()];
    task.approved = false;
    let kinds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let kinds_clone = Arc::clone(&kinds);
    let result = agent.run(
        task,
        RunOpts {
            on_event: Some(Box::new(move |kind, _| {
                kinds_clone.lock().unwrap().push(kind.to_string());
            })),
            ..Default::default()
        },
    );
    assert_eq!(result.tool_calls.len(), 1);
    let call = &result.tool_calls[0];
    assert!(!call.success);
    assert!(call.result_preview.contains("needs human approval"));
    assert!(result.success);
    assert_eq!(result.output, "could not compute");
    assert!(kinds.lock().unwrap().contains(&"tool_blocked".to_string()));
}

#[test]
fn flagged_tool_runs_with_approval() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        CALC_ACTION.to_string(),
        "FINAL: 42".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let mut task = Task::new("what is 6 times 7");
    task.needs_approval = vec!["calculator".to_string()];
    task.approved = true;
    let result = agent.run(task, RunOpts::default());
    assert!(result.tool_calls[0].success);
    assert_eq!(result.tool_calls[0].result_preview, "42");
    assert_eq!(result.output, "42");
}

#[test]
fn blocked_call_does_not_count_toward_same_error_exit() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        CALC_ACTION.to_string(),
        CALC_ACTION.to_string(),
        "FINAL: done without it".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let mut task = Task::new("what is 6 times 7");
    task.needs_approval = vec!["calculator".to_string()];
    let result = agent.run(task, RunOpts::default());
    assert!(result.success);
    assert_eq!(result.output, "done without it");
    assert_eq!(result.tool_calls.len(), 2);
    assert!(result.tool_calls.iter().all(|c| !c.success));
}

#[test]
fn unknown_tool_is_a_failed_call_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec![
            r#"ACTION: {"tool": "no_such_tool", "args": {}}"#,
            "FINAL: moved on",
            "CHECK: PASS",
        ],
        Task::new("do something"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "moved on");
    assert_eq!(result.tool_calls.len(), 1);
    assert!(!result.tool_calls[0].success);
    assert!(result.tool_calls[0].result_preview.contains("Unknown tool"));
}

#[test]
fn malformed_action_gets_a_nudge_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let (_agent, _client, result) = run_with_client(
        dir.path(),
        vec!["ACTION: {not json", CALC_ACTION, "FINAL: 42", "CHECK: PASS"],
        Task::new("what is 6 times 7"),
        RunOpts::default(),
    );
    assert!(result.success);
    assert_eq!(result.output, "42");
    assert_eq!(result.tool_calls.len(), 1);
}

#[test]
fn resume_continues_from_saved_steps() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        "FINAL: 42".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let resume_state = json!({
        "run_id": "run-resume",
        "agent": "TestAgent",
        "task": {"description": "what is 6 times 7", "context": {}, "needs_approval": [], "approved": false, "id": "abc"},
        "steps": [
            {"tool": "calculator", "args": {"expression": "6 * 7"}, "success": true, "result_preview": "42", "elapsed_ms": 1.0}
        ],
        "plan": null,
        "status": "running",
        "answer": null,
        "reason": null,
    });
    let result = agent.run(
        Task::new("what is 6 times 7"),
        RunOpts {
            run_id: Some("run-resume".to_string()),
            resume_state: Some(resume_state),
            ..Default::default()
        },
    );
    assert!(result.success);
    assert_eq!(result.output, "42");
    // The resumed step counts: one prior call, no new tool call.
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].tool, "calculator");
}

#[test]
fn loop_opts_state_hook_sees_running_and_finished() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(FakeClient::new(vec![
        CALC_ACTION.to_string(),
        "FINAL: 42".to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agent = make_agent(dir.path(), Arc::clone(&client), "");
    let statuses: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let statuses_clone = Arc::clone(&statuses);
    let _ = agent.run(
        Task::new("what is 6 times 7"),
        RunOpts {
            state_hook: Some(Box::new(move |snap| {
                statuses_clone.lock().unwrap().push(snap.status.clone());
            })),
            ..Default::default()
        },
    );
    let statuses = statuses.lock().unwrap();
    assert!(statuses.contains(&"running".to_string()));
    assert_eq!(statuses.last().map(|s| s.as_str()), Some("finished"));
}
