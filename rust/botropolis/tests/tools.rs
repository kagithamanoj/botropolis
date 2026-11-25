//! Builtin tool tests: one per tool group, mirroring the Python
//! tests in test_tools.py, test_math_tool.py, test_memory.py,
//! test_jobs.py, and test_consult.py.

use botropolis::agent::{Agent, RunOpts};
use botropolis::memory::{set_memory_store, MemoryStore};
use botropolis::models::ScriptedClient;
use botropolis::registry::AgentRegistry;
use botropolis::schemas::Task;
use botropolis::tools::{
    builtin, consult::set_consult_registry, dispatch, jobs, math, memory_tools, ToolCtx,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_workspace() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::env::set_var("BOTROPOLIS_WORKSPACE", ws.to_string_lossy().to_string());
    std::env::remove_var("BOTROPOLIS_MEMORY_DB");
    std::env::remove_var("BOTROPOLIS_JOBS_FEED_FILE");
    std::env::remove_var("BOTROPOLIS_APPLICATIONS_LOG");
    (dir, guard)
}

fn ctx() -> ToolCtx {
    ToolCtx::new("coder")
}

#[test]
fn shell_runs_and_denylist_blocks() {
    let (_dir, _guard) = temp_workspace();
    let out = builtin::shell(&json!({"command": "echo hello"})).unwrap();
    assert_eq!(out["returncode"], json!(0));
    assert!(out["stdout"].as_str().unwrap().contains("hello"));
    let err = builtin::shell(&json!({"command": "rm -rf /"})).unwrap_err();
    assert!(err.contains("Blocked command"));
    let err = builtin::shell(&json!({"command": "echo hi; rm -rf /tmp/x"})).unwrap_err();
    assert!(err.contains("Blocked command"));
}

#[test]
fn shell_reports_failure_without_raising() {
    let (_dir, _guard) = temp_workspace();
    let out = builtin::shell(&json!({"command": "exit 3"})).unwrap();
    assert_eq!(out["returncode"], json!(3));
}

#[test]
fn file_tools_roundtrip_and_escape_is_blocked() {
    let (_dir, _guard) = temp_workspace();
    let written =
        builtin::write_file(&json!({"path": "notes/todo.txt", "content": "buy milk"})).unwrap();
    assert_eq!(written["path"], json!("notes/todo.txt"));
    assert_eq!(written["bytes"], json!(8));
    let read = builtin::read_file(&json!({"path": "notes/todo.txt"})).unwrap();
    assert_eq!(read, json!("buy milk"));
    let listed = builtin::list_dir(&json!({"path": "notes"})).unwrap();
    assert_eq!(listed["entries"], json!(["todo.txt"]));
    assert!(builtin::read_file(&json!({"path": "../secret.txt"})).is_err());
    assert!(builtin::read_file(&json!({"path": "/etc/passwd"})).is_err());
    assert!(builtin::read_file(&json!({"path": "nope.txt"})).is_err());
    assert!(builtin::list_dir(&json!({"path": "notes/todo.txt"})).is_err());
}

#[test]
fn current_time_is_iso_utc() {
    let out = builtin::current_time(&json!({})).unwrap();
    let text = out.as_str().unwrap();
    assert!(text.contains('T'));
    assert!(text.ends_with("+00:00"));
}

#[test]
fn notebook_append_and_read() {
    let (_dir, _guard) = temp_workspace();
    let empty = builtin::notes_read(&json!({})).unwrap();
    assert_eq!(empty["notes"], json!(""));
    builtin::notes_append(&json!({"text": "Deploys freeze on Fridays"})).unwrap();
    builtin::notes_append(&json!({"text": "Second note"})).unwrap();
    let read = builtin::notes_read(&json!({})).unwrap();
    let notes = read["notes"].as_str().unwrap();
    assert!(notes.contains("Deploys freeze on Fridays"));
    assert!(notes.contains("Second note"));
    let blank = builtin::notes_append(&json!({"text": "   "})).unwrap();
    assert!(blank["error"].as_str().unwrap().contains("empty"));
}

#[test]
fn web_fetch_failure_is_a_result_not_an_exception() {
    let out = builtin::web_fetch(&json!({"url": "http://127.0.0.1:1/nope"})).unwrap();
    assert_eq!(out["url"], json!("http://127.0.0.1:1/nope"));
    assert!(out["error"].as_str().unwrap().contains("fetch failed"));
}

#[test]
fn web_search_returns_the_query_shape() {
    // Whether the network answers or not, the shape holds: the query
    // echoes back and results is a list.
    let out =
        builtin::web_search(&json!({"query": "zxqvkj botropolis", "num_results": 2})).unwrap();
    assert_eq!(out["query"], json!("zxqvkj botropolis"));
    assert!(out["results"].is_array());
}

#[test]
fn calculator_values() {
    let c = ctx();
    assert_eq!(
        dispatch("calculator", &json!({"expression": "6 * 7"}), &c).unwrap(),
        json!(42)
    );
    assert_eq!(
        dispatch("calculator", &json!({"expression": "2 + 3 * 4"}), &c).unwrap(),
        json!(14)
    );
    assert_eq!(
        dispatch("calculator", &json!({"expression": "10 / 4"}), &c).unwrap(),
        json!(2.5)
    );
    assert!(dispatch("calculator", &json!({"expression": "1 / 0"}), &c).is_err());
    assert!(dispatch("calculator", &json!({"expression": "2 +"}), &c).is_err());
}

#[test]
fn math_eval_and_solve() {
    let c = ctx();
    let out = math::math(&json!({"operation": "eval", "expression": "2 + 3 * 4"})).unwrap();
    assert_eq!(out["operation"], json!("eval"));
    assert_eq!(out["value"], json!(14.0));
    let out =
        math::math(&json!({"operation": "solve", "expression": "x^2 - 5*x + 6 = 0"})).unwrap();
    assert_eq!(out["solutions"], json!(["2", "3"]));
    assert_eq!(out["values"], json!([2.0, 3.0]));
    let out = math::math(&json!({"operation": "solve", "expression": "2*x + 4"})).unwrap();
    assert_eq!(out["solutions"], json!(["-2"]));
    let _ = c;
}

#[test]
fn math_polynomial_ops() {
    let out = math::math(&json!({"operation": "diff", "expression": "x^2 + 3*x"})).unwrap();
    assert_eq!(out["result"], json!("2*x + 3"));
    let out = math::math(&json!({"operation": "expand", "expression": "(x + 1)^2"})).unwrap();
    assert_eq!(out["result"], json!("x^2 + 2*x + 1"));
    let out = math::math(&json!({"operation": "factor", "expression": "x^2 - 5*x + 6"})).unwrap();
    assert_eq!(out["result"], json!("(x - 2)*(x - 3)"));
    let out = math::math(&json!({"operation": "simplify", "expression": "2*x + 3*x"})).unwrap();
    assert_eq!(out["result"], json!("5*x"));
    let out = math::math(&json!({"operation": "series", "expression": "x^2", "terms": 3})).unwrap();
    assert_eq!(out["result"], json!("x^2"));
    let out =
        math::math(&json!({"operation": "limit", "expression": "x^2", "point": "2"})).unwrap();
    assert_eq!(out["value"], json!(4.0));
    let out = math::math(&json!({"operation": "eval", "expression": "sin(0)"})).unwrap();
    assert_eq!(out["value"], json!(0.0));
    let out = math::math(
        &json!({"operation": "integrate", "expression": "x^2", "lower": "0", "upper": "1"}),
    )
    .unwrap();
    assert!((out["value"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
    let out = math::math(&json!({"operation": "integrate", "expression": "2*x"})).unwrap();
    assert_eq!(out["result"], json!("x^2"));
}

#[test]
fn math_errors_are_clear() {
    assert!(math::math(&json!({"operation": "teleport", "expression": "x"})).is_err());
    let err = math::math(&json!({"operation": "eval", "expression": "import os"})).unwrap_err();
    assert!(err.contains("not allowed"));
    assert!(
        math::math(&json!({"operation": "eval", "expression": "x + 1", "variable": "w"})).is_err()
    );
    assert!(
        math::math(&json!({"operation": "integrate", "expression": "x", "lower": "0"})).is_err()
    );
    assert!(math::math(&json!({"operation": "limit", "expression": "x"})).is_err());
    let err = math::math(&json!({"operation": "diff", "expression": "sin(x)"})).unwrap_err();
    assert!(err.contains("polynomial"));
    let err = math::math(&json!({"operation": "solve", "expression": "x^3 + x = 0"})).unwrap_err();
    assert!(err.contains("degree 2"));
}

#[test]
fn memory_save_and_recall() {
    let (_dir, _guard) = temp_workspace();
    set_memory_store(Some(MemoryStore::in_memory()));
    let saved = memory_tools::memory_save(
        &json!({"fact": "Deploys freeze on Fridays", "importance": 4, "agent": "scheduler"}),
    )
    .unwrap();
    assert_eq!(saved["saved"], json!(true));
    assert_eq!(saved["memory"]["agent"], json!("scheduler"));
    let found = memory_tools::memory_recall(&json!({"query": "deploy freeze"})).unwrap();
    assert_eq!(found["count"], json!(1));
    assert_eq!(
        found["memories"][0]["text"],
        json!("Deploys freeze on Fridays")
    );
    let none = memory_tools::memory_recall(&json!({"query": "quantum banana"})).unwrap();
    assert_eq!(none["count"], json!(0));
    let empty = memory_tools::memory_save(&json!({"fact": "   "})).unwrap();
    assert!(empty["error"].as_str().unwrap().contains("empty"));
    set_memory_store(None);
}

#[test]
fn memory_save_is_agent_aware_through_dispatch() {
    let (_dir, _guard) = temp_workspace();
    set_memory_store(Some(MemoryStore::in_memory()));
    let ctx = ToolCtx::new("scheduler");
    // The loop injects the calling agent's name at dispatch; calling
    // execute_tool_call the way the loop does records who saved it.
    let call = botropolis::tool_loop::execute_tool_call(
        &["memory_save".to_string()],
        "memory_save",
        &json!({"fact": "The freeze starts at 5pm", "importance": 5}),
        &ctx,
    );
    assert!(call.success);
    let saved: serde_json::Value = serde_json::from_str(&call.result_preview).unwrap();
    assert_eq!(saved["memory"]["agent"], json!("scheduler"));
    set_memory_store(None);
}

#[test]
fn jobs_search_and_apply_with_stubbed_feed() {
    let (dir, _guard) = temp_workspace();
    let feed = dir.path().join("feed.json");
    std::fs::write(
        &feed,
        r#"[{"legal": "RemoteOK feed"}, {"id": 101, "position": "Senior Python Engineer", "company": "Acme Corp", "tags": ["python", "django"], "location": "Remote", "url": "https://remoteok.com/remote-jobs/101", "date": "2026-11-20"}, {"id": 102, "position": "Python Developer", "company": "Initech", "tags": ["python"], "location": "Remote", "url": "https://remoteok.com/remote-jobs/102", "date": "2026-11-21"}]"#,
    )
    .unwrap();
    std::env::set_var(
        "BOTROPOLIS_JOBS_FEED_FILE",
        feed.to_string_lossy().to_string(),
    );
    let log = dir.path().join("applications.jsonl");
    std::env::set_var(
        "BOTROPOLIS_APPLICATIONS_LOG",
        log.to_string_lossy().to_string(),
    );
    let found = jobs::job_search(&json!({"keywords": "python"})).unwrap();
    assert_eq!(found["count"], json!(2));
    assert_eq!(found["jobs"][0]["company"], json!("Acme Corp"));
    let none = jobs::job_search(&json!({"keywords": "cobol"})).unwrap();
    assert_eq!(none["count"], json!(0));

    std::fs::write(
        dir.path().join("workspace").join("resume.txt"),
        "Eval resume.",
    )
    .unwrap();
    let applied = jobs::job_apply(&json!({
        "title": "Senior Python Engineer",
        "company": "Acme Corp",
        "url": "https://remoteok.com/remote-jobs/101",
        "resume": "resume.txt",
    }))
    .unwrap();
    assert_eq!(applied["status"], json!("prepared"));
    assert!(applied["apply_url"]
        .as_str()
        .unwrap()
        .contains("remote-jobs/101"));
    let again = jobs::job_apply(&json!({
        "title": "Senior Python Engineer",
        "company": "Acme Corp",
        "url": "https://remoteok.com/remote-jobs/101",
        "resume": "resume.txt",
    }))
    .unwrap();
    assert_eq!(again["status"], json!("duplicate"));
    let log_text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(log_text.lines().count(), 1);
    assert!(log_text.contains("Acme Corp"));
    let missing = jobs::job_apply(&json!({
        "title": "X", "company": "Y", "url": "https://example.com", "resume": "nope.txt",
    }));
    assert!(missing.is_err());
}

#[test]
fn jobs_search_broken_feed_is_loud() {
    let (dir, _guard) = temp_workspace();
    let feed = dir.path().join("bad.json");
    std::fs::write(&feed, "not json").unwrap();
    std::env::set_var(
        "BOTROPOLIS_JOBS_FEED_FILE",
        feed.to_string_lossy().to_string(),
    );
    let err = jobs::job_search(&json!({"keywords": "python"})).unwrap_err();
    assert!(err.contains("job search failed"));
}

fn spec_yaml(name: &str, department: &str, toolkit: &str) -> String {
    format!(
        "name: {name}\ntitle: {name}\ndepartment: {department}\nspecialty: testing\nmodel: stub\nsystem_prompt: You are a test agent.\ntoolkit: {toolkit}\n"
    )
}

#[test]
fn consult_asks_a_specialist() {
    let (_dir, _guard) = temp_workspace();
    let client = Arc::new(ScriptedClient::new(vec![
        r#"ACTION: {"tool": "consult", "args": {"agent": "specialist", "question": "When does the staging freeze start?"}}"#.to_string(),
        r#"{"summary": "The staging freeze starts Fridays at 5pm."}"#.to_string(),
        r#"FINAL: {"summary": "The staging freeze starts Fridays at 5pm, per the specialist."}"#.to_string(),
        "CHECK: PASS".to_string(),
    ]));
    let agents_dir = tempfile::tempdir().unwrap();
    let dept = agents_dir.path().join("research");
    std::fs::create_dir(&dept).unwrap();
    std::fs::write(
        dept.join("specialist.yaml"),
        spec_yaml("specialist", "research", "[]"),
    )
    .unwrap();
    let registry =
        Arc::new(AgentRegistry::load_from(agents_dir.path(), Some(client.clone())).unwrap());
    let asker_dir = tempfile::tempdir().unwrap();
    let asker_path = asker_dir.path().join("asker.yaml");
    std::fs::write(&asker_path, spec_yaml("asker", "research", "[consult]")).unwrap();
    let asker = Agent::load(&asker_path, Some(client)).unwrap();
    let result = asker.run(
        Task::new("When does the staging freeze start? Ask the specialist, then tell me."),
        RunOpts {
            registry: Some(Arc::clone(&registry)),
            ..Default::default()
        },
    );
    assert!(result.success);
    assert!(result.output.contains("Fridays at 5pm"));
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].tool, "consult");
    assert!(result.tool_calls[0].success);
}

#[test]
fn consult_guards() {
    let (_dir, _guard) = temp_workspace();
    // Self consult is refused.
    let ctx = ToolCtx::new("coder");
    let out = dispatch(
        "consult",
        &json!({"agent": "coder", "question": "What do you think?"}),
        &ctx,
    )
    .unwrap();
    assert!(out["error"]
        .as_str()
        .unwrap()
        .contains("cannot consult yourself"));
    // Unknown agent is an error the model can see.
    let out = dispatch(
        "consult",
        &json!({"agent": "nobody", "question": "Hello?"}),
        &ctx,
    )
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("No agent named"));
    // Depth one cannot consult further.
    ctx.run_context.borrow_mut().depth = 1;
    let out = dispatch(
        "consult",
        &json!({"agent": "reviewer", "question": "Hello?"}),
        &ctx,
    )
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("do not chain"));
    // Budget of three is enforced.
    ctx.run_context.borrow_mut().depth = 0;
    ctx.run_context.borrow_mut().consults_used = 3;
    let out = dispatch(
        "consult",
        &json!({"agent": "reviewer", "question": "Hello?"}),
        &ctx,
    )
    .unwrap();
    assert!(out["error"].as_str().unwrap().contains("budget used up"));
    set_consult_registry(None);
}

#[test]
fn gws_draft_validates_the_address_without_a_cli_call() {
    let out = botropolis::tools::gws::gmail_draft(
        &json!({"to": "not-an-address", "subject": "hi", "body": "hello"}),
    )
    .unwrap();
    assert!(out["error"]
        .as_str()
        .unwrap()
        .contains("Not a valid recipient"));
}

#[test]
fn dispatch_unknown_tool_errors() {
    let c = ctx();
    assert!(dispatch("no_such_tool", &json!({}), &c).is_err());
}

#[test]
fn tool_catalog_covers_every_spec_toolkit() {
    let registry = AgentRegistry::load().unwrap();
    for agent in registry.list_all() {
        for tool in &agent.toolkit {
            assert!(botropolis::tools::known_tool(tool), "{tool}");
        }
    }
}
