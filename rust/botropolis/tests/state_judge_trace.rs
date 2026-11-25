//! Run state, resume, task map, ledger, judge, and trace tests.
//! Mirrors the Python tests in test_runstate.py, test_taskmap.py,
//! test_judge.py, test_trace.py, test_ledger.py, and the resume
//! tests in test_loop_plan_check.py.

use botropolis::agent::{Agent, RunOpts};
use botropolis::evidence::run_check;
use botropolis::judge::{parse_verdict, report_text, run_judge, JUDGE_AGENT_NAME};
use botropolis::ledger::Ledger;
use botropolis::models::{ModelClient, ModelError, ScriptedClient, Usage};
use botropolis::registry::AgentRegistry;
use botropolis::runstate::{task_from_state, RunStore};
use botropolis::schemas::{Contract, ContractCheck, Task};
use botropolis::taskmap::{map_path, plan_steps, read_map, render_map, write_map};
use botropolis::trace::{build_trace, ledger_db_path, render_text};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Tests run in parallel and env vars are process-global.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("botropolis-rust-tests")
        .join(name)
        .join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn state_fixture() -> Value {
    json!({
        "run_id": "run-1",
        "agent": "TestAgent",
        "task": {
            "description": "Work out 6 times 7 and 8 times 9",
            "context": {"note": "kept", "chat_history": "earlier talk"},
        },
        "contract": {
            "goal": "Work out the sums",
            "done_when": ["both sums worked out"],
            "exclusions": ["no guessing"],
            "checks": [],
        },
        "steps": [
            {"tool": "calculator", "args": {"expression": "6 * 7"}, "success": true, "result_preview": "42", "elapsed_ms": 1.0},
        ],
        "plan": "PLAN: Work out 6 times 7.\nThen work out 8 times 9.",
        "status": "finished",
        "answer": "42 and 72.",
    })
}

/// Scripted stand-in that fails the model call after `fail_after`
/// successful replies, to simulate a crash mid-run.
struct CrashingClient {
    replies: Mutex<Vec<String>>,
    calls: Mutex<usize>,
    fail_after: usize,
}

impl CrashingClient {
    fn new(replies: Vec<String>, fail_after: usize) -> Self {
        CrashingClient {
            replies: Mutex::new(replies),
            calls: Mutex::new(0),
            fail_after,
        }
    }
}

impl ModelClient for CrashingClient {
    fn chat(
        &self,
        _model_id: &str,
        _messages: &[botropolis::models::ChatMessage],
    ) -> Result<String, ModelError> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls > self.fail_after {
            return Err(ModelError::Provider("connection lost".to_string()));
        }
        let replies = self.replies.lock().unwrap();
        Ok(replies[(*calls - 1).min(replies.len().saturating_sub(1))].clone())
    }

    fn provider_for(&self, _model_id: &str) -> String {
        "stub".to_string()
    }

    fn last_usage(&self) -> Usage {
        Usage::default()
    }
}

fn make_agent(dir: &std::path::Path, client: Arc<dyn ModelClient>) -> Agent {
    let path = dir.join("testagent.yaml");
    std::fs::write(
        &path,
        "name: TestAgent\ntitle: Tester\ndepartment: research\nspecialty: testing\nmodel: stub\nsystem_prompt: You are a test agent.\noutput_schema: agent_output\ntoolkit: [calculator]\n",
    )
    .unwrap();
    Agent::load(&path, Some(client)).unwrap()
}

fn default_opts() -> RunOpts {
    RunOpts {
        token_budget: None,
        time_budget_seconds: None,
        run_id: None,
        resume_state: None,
        map_text: None,
        run_context: None,
        registry: None,
        ledger: None,
        store: None,
        on_event: None,
        state_hook: None,
    }
}

#[test]
fn runstore_roundtrip_and_missing_run() {
    let dir = temp_dir("runstore");
    let store = RunStore::new(Some(dir.clone()));
    let state = state_fixture();
    store.save(&state).unwrap();
    assert!(store.exists("run-1"));
    let loaded = store.load("run-1").unwrap();
    assert_eq!(loaded["answer"], json!("42 and 72."));
    assert!(loaded["updated_at"].as_f64().unwrap() > 0.0);
    assert_eq!(
        store.load("nope"),
        Err("No saved run with id nope".to_string())
    );
    assert!(!store.exists("nope"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn task_from_state_rebuilds_task_and_contract() {
    let state = state_fixture();
    let task = task_from_state(&state);
    assert_eq!(task.description, "Work out 6 times 7 and 8 times 9");
    assert_eq!(
        task.id, "resumed",
        "no id in the saved task defaults to resumed"
    );
    assert_eq!(
        task.context.extra.get("note").and_then(|v| v.as_str()),
        Some("kept"),
        "extra context keys land in task context"
    );
    assert!(
        !task.context.extra.contains_key("chat_history"),
        "chat_history is a named context field, not an extra"
    );
    assert_eq!(task.context.chat_history, "earlier talk");
    let contract = task.contract.unwrap();
    assert_eq!(contract.goal, "Work out the sums");
    assert_eq!(contract.done_when, vec!["both sums worked out"]);
}

#[test]
fn render_map_contents() {
    let map = render_map(&state_fixture());
    assert!(map.contains("# Task map: run-1"));
    assert!(map.contains("## Goal"));
    assert!(map.contains("Work out the sums"));
    assert!(map.contains("- both sums worked out"));
    assert!(map.contains("- [x] Work out 6 times 7."));
    assert!(map.contains("- [ ] Then work out 8 times 9."));
    assert!(map.contains("calculator"));
    assert!(map.contains("42"));
    assert!(map.contains("## Answer"));
    assert!(map.contains("42 and 72."));
}

#[test]
fn plan_steps_variants() {
    assert_eq!(
        plan_steps(Some("PLAN: First.\n1. Second\n- Third")),
        vec!["First.", "Second", "Third"]
    );
    assert!(plan_steps(None).is_empty());
    assert!(plan_steps(Some("")).is_empty());
}

#[test]
fn map_write_read_and_fallback() {
    let dir = temp_dir("taskmap");
    let store = RunStore::new(Some(dir.clone()));
    let state = state_fixture();
    store.save(&state).unwrap();
    write_map(&store, &state);
    let path = map_path(&store, "run-1");
    assert!(path.is_file());
    let read = read_map(&store, "run-1").unwrap();
    assert!(read.contains("# Task map: run-1"));
    std::fs::remove_file(&path).unwrap();
    let fallback = read_map(&store, "run-1").unwrap();
    assert!(fallback.contains("# Task map: run-1"));
    assert!(read_map(&store, "nope").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ledger_record_replay_and_runs() {
    let ledger = Ledger::in_memory();
    ledger
        .record("run-a", "task_assigned", "TestAgent", json!({"x": 1}))
        .unwrap();
    ledger
        .record(
            "run-a",
            "tool_called",
            "TestAgent",
            json!({"tool": "calculator"}),
        )
        .unwrap();
    ledger
        .record("run-b", "task_assigned", "TestAgent", json!({}))
        .unwrap();
    let events = ledger.events("run-a").unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, "task_assigned");
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].kind, "tool_called");
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[0].payload["x"], json!(1));
    let runs = ledger.runs().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().any(|r| r.run_id == "run-a" && r.events == 2));
}

#[test]
fn judge_verdict_parsing() {
    assert_eq!(parse_verdict("VERDICT: PASS"), Some((true, String::new())));
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
fn judge_report_text_caps_and_labels() {
    let text = report_text(
        "Summary here.",
        &[
            ("AgentA".to_string(), "did the first half".to_string()),
            ("AgentB".to_string(), "did the second half".to_string()),
        ],
    );
    assert!(text.contains("Summary here."));
    assert!(text.contains("[AgentA] did the first half"));
    assert!(text.contains("[AgentB] did the second half"));
    let long = report_text(&"x".repeat(5000), &[]);
    assert!(long.chars().count() <= 4000 + "[truncated]".len() + 10);
    assert!(long.contains("[truncated]"));
}

#[test]
fn run_judge_pass_fail_and_skipped() {
    let registry = AgentRegistry::load().unwrap();
    let reviewer = registry.get(JUDGE_AGENT_NAME).unwrap();
    let contract = Contract {
        goal: "Write a short history of the abacus".to_string(),
        scope: vec![],
        exclusions: vec!["modern calculators".to_string()],
        done_when: vec!["covers the origins".to_string()],
        checks: vec![],
    };
    let report = "The abacus began in Mesopotamia.";

    reviewer.set_client(Arc::new(ScriptedClient::new(vec![
        "VERDICT: PASS".to_string()
    ])));
    let outcome = run_judge(&registry, &contract, report);
    assert_eq!(outcome.verdict, "pass");

    reviewer.set_client(Arc::new(ScriptedClient::new(vec![
        "VERDICT: FAIL\nThe report never covers the origins.".to_string(),
    ])));
    let outcome = run_judge(&registry, &contract, report);
    assert_eq!(outcome.verdict, "fail");
    assert!(outcome.reason.contains("origins"));

    reviewer.set_client(Arc::new(ScriptedClient::new(vec![
        "Looks fine to me.".to_string()
    ])));
    let outcome = run_judge(&registry, &contract, report);
    assert_eq!(outcome.verdict, "skipped");
    assert!(outcome.reason.contains("could not be parsed"));
}

#[test]
fn run_judge_skipped_without_reviewer() {
    let dir = temp_dir("no-reviewer");
    let agents_dir = dir.join("agents");
    std::fs::create_dir_all(agents_dir.join("research")).unwrap();
    std::fs::write(
        agents_dir.join("research").join("analyst.yaml"),
        "name: Analyst\ntitle: Analyst\ndepartment: research\nspecialty: research\nmodel: stub\nsystem_prompt: You research.\n",
    )
    .unwrap();
    let registry = AgentRegistry::load_from(agents_dir, None).unwrap();
    assert!(registry.get(JUDGE_AGENT_NAME).is_err());
    let contract = Contract {
        goal: "g".to_string(),
        scope: vec![],
        exclusions: vec![],
        done_when: vec![],
        checks: vec![],
    };
    let outcome = run_judge(&registry, &contract, "report");
    assert_eq!(outcome.verdict, "skipped");
    assert!(outcome.reason.contains("no reviewer agent"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn trace_build_render_and_missing_run() {
    let dir = temp_dir("trace");
    let store = RunStore::new(Some(dir.join("runs")));
    let state = state_fixture();
    store.save(&state).unwrap();
    let ledger_path = dir.join("ledger.db");
    let ledger = Ledger::open(&ledger_path).unwrap();
    ledger
        .record(
            "run-1",
            "task_assigned",
            "TestAgent",
            json!({"task": "sums"}),
        )
        .unwrap();
    ledger
        .record(
            "run-1",
            "tool_called",
            "TestAgent",
            json!({"tool": "calculator"}),
        )
        .unwrap();
    let trace = build_trace(&store, "run-1", Some(&ledger_path)).unwrap();
    assert_eq!(trace.run_id, "run-1");
    assert_eq!(trace.task, "Work out 6 times 7 and 8 times 9");
    assert_eq!(
        trace.plan,
        vec!["Work out 6 times 7.", "Then work out 8 times 9."]
    );
    assert_eq!(trace.steps.len(), 1);
    assert_eq!(trace.steps[0].tool, "calculator");
    assert!(trace.steps[0].success);
    assert_eq!(trace.events.len(), 2);
    assert_eq!(trace.events[0].kind, "task_assigned");
    let text = render_text(&trace);
    assert!(text.contains("Trace: run-1"));
    assert!(text.contains("calculator"));
    assert!(text.contains("task_assigned"));
    assert!(text.contains("42 and 72."));
    assert!(build_trace(&store, "nope", Some(&ledger_path)).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn trace_without_ledger_has_no_events() {
    let dir = temp_dir("trace-nodb");
    let store = RunStore::new(Some(dir.join("runs")));
    store.save(&state_fixture()).unwrap();
    let trace = build_trace(&store, "run-1", Some(&dir.join("missing.db"))).unwrap();
    assert!(trace.events.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ledger_db_path_never_creates_files() {
    let _guard = lock_env();
    std::env::remove_var("BOTROPOLIS_LEDGER_DB");
    let found = ledger_db_path();
    if let Some(path) = found {
        assert!(path.is_file(), "ledger_db_path only returns existing files");
    }
    std::env::set_var(
        "BOTROPOLIS_LEDGER_DB",
        std::env::temp_dir().join("definitely-not-there.db"),
    );
    assert_eq!(ledger_db_path(), None);
    std::env::remove_var("BOTROPOLIS_LEDGER_DB");
}

#[test]
fn resume_after_crash() {
    let dir = temp_dir("resume");
    let store = RunStore::new(Some(dir.join("runs")));
    let task = Task::new("Work out 6 times 7, then answer 42.");

    // First run: the model answers with a calculator action, then the
    // connection drops on the next call. The run fails, but the step
    // is saved to disk with a task map beside it.
    let crashing: Arc<dyn ModelClient> = Arc::new(CrashingClient::new(
        vec![r#"ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}"#.to_string()],
        1,
    ));
    let agent = make_agent(&dir, crashing);
    let mut opts = default_opts();
    opts.run_id = Some("resume-1".to_string());
    opts.store = Some(store.clone());
    let result = agent.run(task.clone(), opts);
    assert!(!result.success, "the crashed run fails");
    assert!(store.exists("resume-1"));
    let map = read_map(&store, "resume-1").unwrap();
    assert!(map.contains("calculator"));
    assert!(map.contains("- [x] Work out 6 times 7.") || map.contains("6 * 7"));

    // Second run: a fresh client resumes from the saved state and
    // answers. The calculator step is not repeated.
    let saved = store.load("resume-1").unwrap();
    assert_eq!(saved["steps"].as_array().unwrap().len(), 1);
    let resumed: Arc<dyn ModelClient> = Arc::new(CrashingClient::new(
        vec!["FINAL: 42".to_string(), "CHECK: PASS".to_string()],
        usize::MAX,
    ));
    let agent2 = make_agent(&dir, resumed);
    let mut opts2 = default_opts();
    opts2.run_id = Some("resume-1".to_string());
    opts2.resume_state = Some(saved);
    opts2.store = Some(store.clone());
    let result2 = agent2.run(task, opts2);
    assert!(result2.success, "resumed run succeeds: {:?}", result2.error);
    assert_eq!(result2.output, "42");
    assert_eq!(result2.tool_calls.len(), 1, "the saved step is not redone");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ledger_kinds_recorded_on_a_run() {
    let dir = temp_dir("ledger-run");
    let store = RunStore::new(Some(dir.join("runs")));
    let ledger = Ledger::in_memory();
    let client: Arc<dyn ModelClient> = Arc::new(CrashingClient::new(
        vec![
            "PLAN: Work out 6 times 7 with the calculator, then answer.".to_string(),
            r#"ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}"#.to_string(),
            "FINAL: 42".to_string(),
            "CHECK: PASS".to_string(),
        ],
        usize::MAX,
    ));
    let agent = make_agent(&dir, client);
    let task = Task::new("Work out 6 times 7, then answer 42.");
    let mut opts = default_opts();
    opts.run_id = Some("ledger-1".to_string());
    opts.ledger = Some(ledger.clone());
    opts.store = Some(store);
    let result = agent.run(task, opts);
    assert!(result.success, "run succeeds: {:?}", result.error);
    let events = ledger.events("ledger-1").unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    for expected in [
        "task_assigned",
        "plan_recorded",
        "tool_called",
        "answer_checked",
        "output_produced",
    ] {
        assert!(
            kinds.contains(&expected),
            "ledger should carry {expected}, got {kinds:?}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn evidence_checks_pass_fail_and_refused() {
    let passed = run_check(&ContractCheck::new("echo ok"));
    assert!(passed.passed);
    assert_eq!(passed.exit_code, Some(0));
    assert!(passed.output_tail.contains("ok"));

    let failed = run_check(&ContractCheck::new("false"));
    assert!(!failed.passed);
    assert_eq!(failed.exit_code, Some(1));

    let refused = run_check(&ContractCheck::new("rm -rf /"));
    assert!(!refused.passed);
    assert!(refused.output_tail.contains("refused"));
}
