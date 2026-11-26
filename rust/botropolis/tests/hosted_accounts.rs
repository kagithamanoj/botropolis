//! Team account integration tests: signup, signin, API key
//! creation and use, listing without key material, and revocation.
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
) -> (StatusCode, Value, Vec<(String, String)>) {
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
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, headers)
}

/// Auth headers for a team: a session cookie, or an API key.
fn cookie(token: &str) -> (&'static str, String) {
    ("cookie", format!("botropolis_session={token}"))
}

fn key_header(key: &str) -> (&'static str, String) {
    ("x-botropolis-key", key.to_string())
}

async fn signup(app: axum::Router) -> (StatusCode, Value) {
    let (status, body, _) = call(
        app,
        "POST",
        "/signup",
        None,
        Some(json!({"email": "acme@example.com", "password": "correct horse", "name": "Acme"})),
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn signup_then_signin() {
    let (_fx, app) = fixture();
    let (status, body) = signup(app.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["team"]["email"], json!("acme@example.com"));
    assert_eq!(body["team"]["plan"], json!("free"));
    assert!(body["team"]["password_hash"].is_null());
    assert!(body["session_token"].as_str().unwrap().len() > 20);
    let (status, _) = signup(app.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = call(
        app.clone(),
        "POST",
        "/signin",
        None,
        Some(json!({"email": "acme@example.com", "password": "correct horse"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["team"]["email"], json!("acme@example.com"));
    let (status, body, _) = call(
        app.clone(),
        "POST",
        "/signin",
        None,
        Some(json!({"email": "acme@example.com", "password": "wrong password"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["detail"].as_str().unwrap().contains("invalid"));
}

#[tokio::test]
async fn key_lifecycle_through_the_api() {
    let (_fx, app) = fixture();
    let (status, body) = signup(app.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let token = body["session_token"].as_str().unwrap().to_string();
    let (auth_name, auth_value) = cookie(&token);

    // Create a key with the session cookie.
    let (status, body, _) = call(
        app.clone(),
        "POST",
        "/teams/api-keys",
        Some((auth_name, &auth_value)),
        Some(json!({"name": "ci"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let key = body["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("btp_"));
    let key_id = body["id"].as_str().unwrap().to_string();

    // The key works for hosted /ask.
    let (key_name, key_value) = key_header(&key);
    let (status, body, _) = call(
        app.clone(),
        "POST",
        "/ask",
        Some((key_name, &key_value)),
        Some(json!({"request": "research the history of the abacus"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["summary"].as_str().unwrap().contains("abacus"));

    // Listing shows metadata only, never the key.
    let (status, body, _) = call(
        app.clone(),
        "GET",
        "/teams/api-keys",
        Some((auth_name, &auth_value)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let keys = body["api_keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["id"], json!(key_id));
    assert!(keys[0].get("key").is_none());
    assert!(keys[0]["last_used_at"].is_number());

    // Revoke, then the key is rejected.
    let (status, body, _) = call(
        app.clone(),
        "DELETE",
        &format!("/teams/api-keys/{key_id}"),
        Some((auth_name, &auth_value)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], json!(true));
    let (status, _, _) = call(
        app.clone(),
        "POST",
        "/ask",
        Some((key_name, &key_value)),
        Some(json!({"request": "research the abacus"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn key_endpoints_need_auth() {
    let (_fx, app) = fixture();
    let (status, _, _) = call(app.clone(), "GET", "/teams/api-keys", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(
        app.clone(),
        "POST",
        "/teams/api-keys",
        None,
        Some(json!({"name": "ci"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(app.clone(), "DELETE", "/teams/api-keys/key-1", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
