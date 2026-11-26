//! Tenancy integration tests: two tenants cannot read each
//! other's state, hosted mode rejects keyless requests, and
//! single-tenant mode still works exactly as before.
//!
//! The tests point `BOTROPOLIS_DATA_DIR` and
//! `BOTROPOLIS_PLATFORM_DB` at a temp dir, so no real tenant state
//! is created. Env vars are process-global, so the whole file runs
//! under one lock.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use botropolis::server::{build_app, AppState};
use botropolis::tenancy::hash_secret;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    _dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn fixture(hosted: bool) -> (Fixture, axum::Router) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = dir.path().join("platform.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE api_keys (id TEXT PRIMARY KEY, team_id TEXT NOT NULL, key_hash TEXT NOT NULL, created_at REAL NOT NULL, revoked_at REAL);
         CREATE TABLE sessions (id TEXT PRIMARY KEY, team_id TEXT NOT NULL, token_hash TEXT NOT NULL, created_at REAL NOT NULL, expires_at REAL NOT NULL);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO api_keys (id, team_id, key_hash, created_at, revoked_at) VALUES (?1, ?2, ?3, 0, NULL)",
        rusqlite::params!["key-a", "team-a", hash_secret("key-for-team-a")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO api_keys (id, team_id, key_hash, created_at, revoked_at) VALUES (?1, ?2, ?3, 0, NULL)",
        rusqlite::params!["key-b", "team-b", hash_secret("key-for-team-b")],
    )
    .unwrap();
    drop(conn);
    std::env::set_var("BOTROPOLIS_DATA_DIR", &data_dir);
    std::env::set_var("BOTROPOLIS_PLATFORM_DB", &db_path);
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    let state = Arc::new(AppState::load_with(hosted).unwrap());
    let app = build_app(state);
    (
        Fixture {
            _dir: dir,
            _guard: guard,
        },
        app,
    )
}

async fn post(
    app: axum::Router,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("x-botropolis-key", key);
    }
    let response = app
        .oneshot(
            builder
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

async fn get(app: axum::Router, path: &str, key: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(path);
    if let Some(key) = key {
        builder = builder.header("x-botropolis-key", key);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
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
async fn hosted_mode_rejects_keyless_requests() {
    let (_fx, app) = fixture(true);
    let (status, _) = post(
        app.clone(),
        "/ask",
        None,
        json!({"request": "research the abacus"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get(app.clone(), "/analytics", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get(app.clone(), "/notebook", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post(
        app.clone(),
        "/ask",
        Some("wrong-key"),
        json!({"request": "research the abacus"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hosted_mode_accepts_a_valid_team_key() {
    let (_fx, app) = fixture(true);
    let (status, body) = post(
        app.clone(),
        "/ask",
        Some("key-for-team-a"),
        json!({"request": "research the history of the abacus"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["summary"].as_str().unwrap().contains("abacus"));
}

#[tokio::test]
async fn tenants_cannot_read_each_others_usage_or_notebook() {
    let (_fx, app) = fixture(true);
    let (status, _) = post(
        app.clone(),
        "/ask",
        Some("key-for-team-a"),
        json!({"request": "research the history of the abacus"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get(app.clone(), "/analytics", Some("key-for-team-a")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totals"]["calls"], json!(1));
    let (status, body) = get(app.clone(), "/analytics", Some("key-for-team-b")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totals"]["calls"], json!(0));
    // Team B's notebook is empty: team A's run wrote nothing there,
    // and team A never touched team B's file.
    let (status, body) = get(app.clone(), "/notebook", Some("key-for-team-b")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], json!(""));
}

#[tokio::test]
async fn single_tenant_mode_still_works_with_no_key() {
    let (_fx, app) = fixture(false);
    let (status, body) = post(
        app.clone(),
        "/ask",
        None,
        json!({"request": "research the history of the abacus"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["summary"].as_str().unwrap().contains("abacus"));
    let (status, body) = get(app.clone(), "/analytics", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totals"]["calls"], json!(1));
}

#[tokio::test]
async fn team_runs_are_scoped_to_the_tenant() {
    let (_fx, app) = fixture(true);
    let (status, body) = post(
        app.clone(),
        "/team",
        Some("key-for-team-b"),
        json!({"request": "draft a short note", "agents": ["coder"], "rounds": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["synthesis"].as_str().unwrap().contains("coder"));
    let (status, body) = get(app.clone(), "/analytics", Some("key-for-team-b")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totals"]["calls"], json!(1));
    let (status, body) = get(app.clone(), "/analytics", Some("key-for-team-a")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totals"]["calls"], json!(0));
}
