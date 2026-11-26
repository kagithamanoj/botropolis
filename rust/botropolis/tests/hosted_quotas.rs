//! Quota integration tests: a team with a tiny free quota can run
//! once, is blocked with 402 on the next run, and can still read
//! its usage.
//!
//! Env vars are process-global, so the whole file runs under one
//! lock, and the platform database lives in a temp dir.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use botropolis::server::{build_app, AppState};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    _dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn fixture() -> (Fixture, axum::Router) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::env::set_var("BOTROPOLIS_DATA_DIR", &data_dir);
    std::env::set_var("BOTROPOLIS_PLATFORM_DB", dir.path().join("platform.db"));
    std::env::set_var("BOTROPOLIS_FREE_REQUESTS_PER_MONTH", "1");
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    let state = Arc::new(AppState::load_with(true).unwrap());
    let app = build_app(state);
    (
        Fixture {
            _dir: dir,
            _guard: guard,
        },
        app,
    )
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    auth: Option<(&str, &str)>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some((name, value)) = auth {
        builder = builder.header(name, value);
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn quota_blocks_runs_but_not_reads() {
    let (_fx, app) = fixture();
    let (status, body) = call(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({"email": "acme@example.com", "password": "correct horse", "name": "Acme"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = body["session_token"].as_str().unwrap().to_string();
    let cookie = format!("botropolis_session={token}");
    let auth = ("cookie", cookie.as_str());

    let (status, body) = call(app.clone(), "GET", "/teams/usage", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["plan"], json!("free"));
    assert_eq!(body["requests"], json!(0));
    assert_eq!(body["quota"]["requests_limit"], json!(1));
    assert_eq!(body["quota"]["exceeded"], json!(false));

    let (status, _) = call(
        app.clone(),
        "POST",
        "/ask",
        Some(auth),
        Some(json!({"request": "research the history of the abacus"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(app.clone(), "GET", "/teams/usage", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requests"], json!(1));
    assert_eq!(body["quota"]["exceeded"], json!(true));
    assert_eq!(body["agents"]["scout"]["calls"], json!(1));

    let (status, body) = call(
        app.clone(),
        "POST",
        "/ask",
        Some(auth),
        Some(json!({"request": "research the abacus again"})),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert!(body["detail"].as_str().unwrap().contains("quota"));

    let (status, body) = call(
        app.clone(),
        "POST",
        "/team",
        Some(auth),
        Some(json!({"request": "draft a note", "agents": ["coder"], "rounds": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert!(body["detail"].as_str().unwrap().contains("quota"));

    // Reads still work past the quota.
    let (status, body) = call(app.clone(), "GET", "/teams/usage", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requests"], json!(1));
    let (status, _) = call(app.clone(), "GET", "/analytics", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    std::env::remove_var("BOTROPOLIS_FREE_REQUESTS_PER_MONTH");
}
