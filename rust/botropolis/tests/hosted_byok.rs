//! BYOK integration tests: storing a provider key needs the master
//! key, listing returns names only, and no key material appears in
//! any API response.
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

fn fixture(with_master_key: bool) -> (Fixture, axum::Router) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::env::set_var("BOTROPOLIS_DATA_DIR", &data_dir);
    std::env::set_var("BOTROPOLIS_PLATFORM_DB", dir.path().join("platform.db"));
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    if with_master_key {
        std::env::set_var("BOTROPOLIS_MASTER_KEY", "11".repeat(32));
    } else {
        std::env::remove_var("BOTROPOLIS_MASTER_KEY");
    }
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

async fn signup_token(app: axum::Router) -> String {
    let (status, body) = call(
        app,
        "POST",
        "/signup",
        None,
        Some(json!({"email": "acme@example.com", "password": "correct horse", "name": "Acme"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["session_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn store_list_and_delete_provider_key() {
    let (_fx, app) = fixture(true);
    let token = signup_token(app.clone()).await;
    let cookie = format!("botropolis_session={token}");
    let auth = ("cookie", cookie.as_str());
    let secret = "sk-team-openai-key-12345";
    let (status, body) = call(
        app.clone(),
        "POST",
        "/teams/provider-keys",
        Some(auth),
        Some(json!({"provider": "openai", "key": secret})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stored"], json!(true));
    assert!(!body.to_string().contains(secret));
    let (status, body) = call(app.clone(), "GET", "/teams/provider-keys", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["providers"], json!(["openai"]));
    assert!(!body.to_string().contains(secret));
    let (status, body) = call(
        app.clone(),
        "DELETE",
        "/teams/provider-keys/openai",
        Some(auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], json!(true));
    let (status, body) = call(app.clone(), "GET", "/teams/provider-keys", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["providers"], json!([]));
    let (status, _) = call(
        app.clone(),
        "DELETE",
        "/teams/provider-keys/openai",
        Some(auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn store_refuses_without_master_key() {
    let (_fx, app) = fixture(false);
    let token = signup_token(app.clone()).await;
    let cookie = format!("botropolis_session={token}");
    let auth = ("cookie", cookie.as_str());
    let (status, body) = call(
        app.clone(),
        "POST",
        "/teams/provider-keys",
        Some(auth),
        Some(json!({"provider": "openai", "key": "sk-team-key"})),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("BOTROPOLIS_MASTER_KEY"));
    let (status, body) = call(app.clone(), "GET", "/teams/provider-keys", Some(auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["providers"], json!([]));
}

#[tokio::test]
async fn unknown_provider_is_rejected() {
    let (_fx, app) = fixture(true);
    let token = signup_token(app.clone()).await;
    let cookie = format!("botropolis_session={token}");
    let auth = ("cookie", cookie.as_str());
    let (status, _) = call(
        app.clone(),
        "POST",
        "/teams/provider-keys",
        Some(auth),
        Some(json!({"provider": "cohere", "key": "some-key"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
