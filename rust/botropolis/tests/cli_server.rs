//! CLI smoke tests and server endpoint tests.
//! Mirrors the Python tests in test_cli.py and the server tests in
//! test_warroom_analytics.py, plus the war-room endpoint behavior.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use botropolis::cli::run as run_cli;
use botropolis::server::{build_app, AppState};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Tests run in parallel and env vars are process-global.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Drop any live model keys so agent runs use the offline stub.
/// The stub is what these tests exercise; no test should ever call
/// a live model. The lock only covers the removal itself; nothing in
/// these tests puts a key back.
fn use_stub() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
}

fn argv(parts: &[&str]) -> Vec<String> {
    let mut out = vec!["botropolis".to_string()];
    out.extend(parts.iter().map(|s| s.to_string()));
    out
}

#[test]
fn roster_lists_agents() {
    use_stub();
    assert_eq!(run_cli(argv(&["roster"])), 0);
}

#[test]
fn roster_filters_by_department() {
    use_stub();
    assert_eq!(run_cli(argv(&["roster", "--department", "research"])), 0);
}

#[test]
fn roster_unknown_department_fails() {
    use_stub();
    assert_eq!(run_cli(argv(&["roster", "--department", "nope"])), 1);
}

#[test]
fn ask_unknown_agent_fails() {
    use_stub();
    assert_eq!(run_cli(argv(&["ask", "NoSuchAgent", "hello"])), 1);
}

#[test]
fn ask_runs_agent() {
    use_stub();
    assert_eq!(run_cli(argv(&["ask", "scout", "What is 6 times 7?"])), 0);
}

#[test]
fn team_runs_and_prints_synthesis() {
    use_stub();
    assert_eq!(
        run_cli(argv(&[
            "team",
            "draft a short note",
            "coder",
            "--rounds",
            "1"
        ])),
        0
    );
}

#[test]
fn team_unknown_agent_fails() {
    use_stub();
    assert_eq!(run_cli(argv(&["team", "do a thing", "NoSuchAgent"])), 1);
}

#[test]
fn evals_subcommand_passes() {
    use_stub();
    assert_eq!(run_cli(argv(&["evals"])), 0);
}

#[test]
fn map_missing_run_fails() {
    use_stub();
    assert_eq!(run_cli(argv(&["map", "no-such-run"])), 1);
}

#[test]
fn trace_missing_run_fails() {
    use_stub();
    assert_eq!(run_cli(argv(&["trace", "no-such-run"])), 1);
}

#[test]
fn resume_missing_run_fails() {
    use_stub();
    assert_eq!(run_cli(argv(&["resume", "no-such-run"])), 1);
}

#[test]
fn serve_help_lists_host_and_hosted_flags() {
    use_stub();
    assert_eq!(run_cli(argv(&["serve", "--help"])), 0);
}

#[test]
fn notebook_reads() {
    use_stub();
    assert_eq!(run_cli(argv(&["notebook"])), 0);
}

fn app() -> axum::Router {
    let state = Arc::new(AppState::load().unwrap());
    build_app(state)
}

async fn get_json(path: &str) -> (StatusCode, Value) {
    let response = app()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn post_json(path: &str, body: Value) -> (StatusCode, Value) {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn health_reports_agent_count() {
    use_stub();
    let (status, body) = get_json("/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], json!("ok"));
    assert_eq!(body["agents"], json!(20));
}

#[tokio::test]
async fn agents_list_and_detail() {
    use_stub();
    let (status, body) = get_json("/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agents"].as_array().unwrap().len(), 20);
    let (status, body) = get_json("/agents/coder").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], json!("coder"));
    let (status, body) = get_json("/agents/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["detail"].as_str().unwrap().contains("Unknown agent"));
}

#[tokio::test]
async fn departments_list() {
    use_stub();
    let (status, body) = get_json("/departments").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["departments"].as_array().unwrap().len(), 10);
}

#[tokio::test]
async fn analytics_and_notebook() {
    use_stub();
    let (status, body) = get_json("/analytics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["totals"].is_object());
    let (status, body) = get_json("/notebook").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["content"].is_string());
}

#[tokio::test]
async fn ask_routes_through_ceo() {
    use_stub();
    let (status, body) = post_json(
        "/ask",
        json!({"request": "research the history of the abacus"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["summary"].as_str().unwrap().contains("abacus"));
    assert_eq!(body["departments_involved"], json!(["research"]));
}

#[tokio::test]
async fn ask_rejects_empty_request() {
    use_stub();
    let (status, _) = post_json("/ask", json!({"request": "  "})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ask_agent_direct() {
    use_stub();
    let (status, body) = post_json(
        "/agents/scout/ask",
        json!({"request": "What is 6 times 7?"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agent_name"], json!("scout"));
    let (status, _) = post_json("/agents/nope/ask", json!({"request": "hi"})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_endpoint() {
    use_stub();
    let (status, body) = post_json(
        "/team",
        json!({"request": "draft a short note", "agents": ["coder"], "rounds": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["synthesis"].as_str().unwrap().contains("coder"));
    let (status, _) = post_json(
        "/team",
        json!({"request": "x", "agents": ["NoSuchAgent"], "rounds": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn agent_stream_ends_with_done() {
    use_stub();
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/agents/scout/ask/stream?request=What%20is%206%20times%207%3F")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("event: result"),
        "stream carries a result: {text}"
    );
    assert!(
        text.contains("event: done"),
        "stream ends with done: {text}"
    );
}

#[tokio::test]
async fn index_and_static_ui() {
    use_stub();
    let response = app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/static/app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
