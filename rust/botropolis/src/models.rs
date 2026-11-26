//! Model client abstraction for Botropolis.
//!
//! Supported providers: openai, anthropic, google (Gemini), ollama
//! (local), and stub (deterministic offline fallback). Configuration
//! comes from the environment:
//!
//! ```text
//! OPENAI_API_KEY      OpenAI API key
//! ANTHROPIC_API_KEY   Anthropic API key
//! GOOGLE_API_KEY      Google AI Studio API key (Gemini)
//! OLLAMA_HOST         Ollama base URL, default http://localhost:11434
//! ```
//!
//! When no key is configured for the requested model, the client falls
//! back to a deterministic offline stub. The stub never claims to be a
//! real model call: every stub response is clearly labeled as offline
//! output. Behavior matches `botropolis/core/models.py`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

use crate::paths::model_registry_path;

pub const PROVIDERS: [&str; 5] = ["openai", "anthropic", "google", "ollama", "stub"];

const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("{0}")]
    Provider(String),
    #[error("provider request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("provider returned an unexpected response: {0}")]
    BadResponse(String),
}

/// One chat message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// Token counts reported by the most recent chat call.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
}

/// A model client: anything that can answer a chat request.
pub trait ModelClient: Send + Sync {
    /// Send chat messages and return the assistant text.
    fn chat(&self, model_id: &str, messages: &[ChatMessage]) -> Result<String, ModelError>;
    /// Which provider would serve this model id.
    fn provider_for(&self, model_id: &str) -> String;
    /// Tokens reported by the most recent chat call.
    fn last_usage(&self) -> Usage;
}

/// One entry in `models/registry.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// The parsed `models/registry.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRegistryFile {
    #[serde(default)]
    pub base_models: Vec<ModelEntry>,
    #[serde(default)]
    pub fine_tunes: Vec<ModelEntry>,
}

/// Load `models/registry.yaml`, returning an empty registry when it is missing.
pub fn load_model_registry(path: Option<&Path>) -> ModelRegistryFile {
    let path: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => model_registry_path(),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return ModelRegistryFile::default(),
    };
    serde_yaml::from_str(&text).unwrap_or_default()
}

/// Find a model entry by id across base_models and fine_tunes.
pub fn find_model_entry(model_id: &str, registry: &ModelRegistryFile) -> Option<ModelEntry> {
    registry
        .base_models
        .iter()
        .chain(registry.fine_tunes.iter())
        .find(|entry| entry.id == model_id)
        .cloned()
}

fn env_key(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "google" => Some("GOOGLE_API_KEY"),
        "ollama" => Some("OLLAMA_HOST"),
        _ => None,
    }
}

// Provider keys for the request running on this thread, set by the
// server from the requesting team's stored keys (BYOK). They win
// over the platform environment keys.
thread_local! {
    static REQUEST_KEYS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Run `f` with these provider keys in scope for this thread.
pub fn with_request_provider_keys<R>(keys: &HashMap<String, String>, f: impl FnOnce() -> R) -> R {
    let previous = REQUEST_KEYS.with(|cell| cell.replace(keys.clone()));
    let out = f();
    REQUEST_KEYS.with(|cell| cell.replace(previous));
    out
}

fn request_key(provider: &str) -> Option<String> {
    REQUEST_KEYS.with(|cell| cell.borrow().get(provider).cloned())
}

/// The key to use for a provider: the requesting team's stored key
/// first, then the platform environment key. None when neither
/// exists, which is when the offline stub takes over.
pub fn key_for_provider(provider: &str) -> Option<String> {
    if let Some(key) = request_key(provider) {
        if !key.is_empty() {
            return Some(key);
        }
    }
    match env_key(provider) {
        Some(var) => env::var(var).ok().filter(|v| !v.is_empty()),
        None => None,
    }
}

fn provider_has_credentials(provider: &str) -> bool {
    match provider {
        "stub" => true,
        "ollama" => true, // local server; reachability is checked at call time
        _ => key_for_provider(provider).is_some(),
    }
}

/// Guess a provider from the model id when the registry has no entry.
/// Unknown ids default to the stub: offline-safe, and every stub
/// response is labeled so the fallback is never silent.
pub fn infer_provider(model_id: &str) -> &'static str {
    let lowered = model_id.to_lowercase();
    if lowered == "stub" {
        return "stub";
    }
    if lowered.starts_with("gpt") || lowered.starts_with("o1") || lowered.starts_with("o3") {
        return "openai";
    }
    if lowered.starts_with("claude") {
        return "anthropic";
    }
    if lowered.starts_with("gemini") {
        return "google";
    }
    "stub"
}

/// Route chat requests to a provider, or to the offline stub.
pub struct RealClient {
    pub default_model: String,
    registry: ModelRegistryFile,
    last_usage: Mutex<Usage>,
    /// Built lazily on the first live HTTP call. A blocking client
    /// owns a runtime, so building one eagerly would make every
    /// stub-only run (and the server, which drops agents inside the
    /// async runtime) carry a runtime it never uses.
    http: Mutex<Option<reqwest::blocking::Client>>,
}

impl RealClient {
    pub fn new(default_model: impl Into<String>) -> Self {
        RealClient {
            default_model: default_model.into(),
            registry: load_model_registry(None),
            last_usage: Mutex::new(Usage::default()),
            http: Mutex::new(None),
        }
    }

    /// The shared HTTP client, built on first use.
    fn http_client(&self) -> reqwest::blocking::Client {
        let mut slot = self.http.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(client) = slot.as_ref() {
            return client.clone();
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        *slot = Some(client.clone());
        client
    }

    /// Pick the best provider for a model id, preferring stub offline.
    pub fn resolve_provider(&self, model_id: &str) -> String {
        if let Some(entry) = find_model_entry(model_id, &self.registry) {
            let provider = if entry.provider.is_empty() {
                "stub".to_string()
            } else {
                entry.provider
            };
            if provider_has_credentials(&provider) {
                return provider;
            }
            return "stub".to_string();
        }
        let provider = infer_provider(model_id);
        if provider_has_credentials(provider) {
            return provider.to_string();
        }
        "stub".to_string()
    }

    fn set_usage(&self, usage: Usage) {
        if let Ok(mut slot) = self.last_usage.lock() {
            *slot = usage;
        }
    }
}

impl ModelClient for RealClient {
    fn chat(&self, model_id: &str, messages: &[ChatMessage]) -> Result<String, ModelError> {
        let model_id = if model_id.is_empty() {
            self.default_model.as_str()
        } else {
            model_id
        };
        self.set_usage(Usage::default());
        let provider = self.resolve_provider(model_id);
        if provider == "stub" {
            return Ok(stub_chat(model_id, messages));
        }
        let (text, usage) = match provider.as_str() {
            "openai" => self.chat_openai(model_id, messages)?,
            "anthropic" => self.chat_anthropic(model_id, messages)?,
            "google" => self.chat_google(model_id, messages)?,
            "ollama" => self.chat_ollama(model_id, messages)?,
            other => return Err(ModelError::Provider(format!("Unknown provider: {other}"))),
        };
        self.set_usage(usage);
        Ok(text)
    }

    fn provider_for(&self, model_id: &str) -> String {
        let model_id = if model_id.is_empty() {
            self.default_model.as_str()
        } else {
            model_id
        };
        self.resolve_provider(model_id)
    }

    fn last_usage(&self) -> Usage {
        self.last_usage.lock().map(|u| *u).unwrap_or_default()
    }
}

impl Default for RealClient {
    fn default() -> Self {
        RealClient::new("stub")
    }
}

fn norm_usage(raw: &serde_json::Value, in_keys: &[&str], out_keys: &[&str]) -> Usage {
    let pick = |keys: &[&str]| -> u64 {
        for key in keys {
            if let Some(v) = raw.get(key).and_then(|v| v.as_u64()) {
                return v;
            }
        }
        0
    };
    Usage {
        input: pick(in_keys),
        output: pick(out_keys),
    }
}

impl RealClient {
    fn chat_openai(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
    ) -> Result<(String, Usage), ModelError> {
        let key = key_for_provider("openai").unwrap_or_default();
        let resp = self
            .http_client()
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(key)
            .json(&serde_json::json!({"model": model_id, "messages": messages}))
            .send()?;
        let data: serde_json::Value = resp
            .json()
            .map_err(|e| ModelError::BadResponse(e.to_string()))?;
        let usage = norm_usage(
            data.get("usage").unwrap_or(&serde_json::Value::Null),
            &["prompt_tokens"],
            &["completion_tokens"],
        );
        let text = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| ModelError::BadResponse("no choices in OpenAI response".into()))?
            .to_string();
        Ok((text, usage))
    }

    fn chat_anthropic(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
    ) -> Result<(String, Usage), ModelError> {
        let key = key_for_provider("anthropic").unwrap_or_default();
        let system: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect();
        let convo: Vec<&ChatMessage> = messages.iter().filter(|m| m.role != "system").collect();
        let mut payload = serde_json::json!({
            "model": model_id,
            "max_tokens": 1024,
            "messages": convo,
        });
        if !system.is_empty() {
            payload["system"] = serde_json::Value::String(system.join("\n"));
        }
        let resp = self
            .http_client()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&payload)
            .send()?;
        let data: serde_json::Value = resp
            .json()
            .map_err(|e| ModelError::BadResponse(e.to_string()))?;
        let usage = norm_usage(
            data.get("usage").unwrap_or(&serde_json::Value::Null),
            &["input_tokens"],
            &["output_tokens"],
        );
        let mut text = String::new();
        if let Some(blocks) = data.get("content").and_then(|c| c.as_array()) {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
            }
        }
        Ok((text, usage))
    }

    fn chat_google(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
    ) -> Result<(String, Usage), ModelError> {
        let key = key_for_provider("google").unwrap_or_default();
        let system: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect();
        let contents: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                let role = if m.role == "user" { "user" } else { "model" };
                serde_json::json!({"role": role, "parts": [{"text": m.content}]})
            })
            .collect();
        let mut payload = serde_json::json!({"contents": contents});
        if !system.is_empty() {
            payload["systemInstruction"] =
                serde_json::json!({"parts": [{"text": system.join("\n")}]});
        }
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model_id}:generateContent"
        );
        let resp = self
            .http_client()
            .post(url)
            .query(&[("key", key)])
            .json(&payload)
            .send()?;
        let data: serde_json::Value = resp
            .json()
            .map_err(|e| ModelError::BadResponse(e.to_string()))?;
        let usage = norm_usage(
            data.get("usageMetadata")
                .unwrap_or(&serde_json::Value::Null),
            &["promptTokenCount"],
            &["candidatesTokenCount"],
        );
        let mut text = String::new();
        if let Some(parts) = data
            .get("candidates")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
        {
            for part in parts {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
        }
        Ok((text, usage))
    }

    fn chat_ollama(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
    ) -> Result<(String, Usage), ModelError> {
        let host = env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| DEFAULT_OLLAMA_HOST.to_string())
            .trim_end_matches('/')
            .to_string();
        let resp = self
            .http_client()
            .post(format!("{host}/api/chat"))
            .json(&serde_json::json!({"model": model_id, "messages": messages, "stream": false}))
            .send()?;
        let data: serde_json::Value = resp
            .json()
            .map_err(|e| ModelError::BadResponse(e.to_string()))?;
        let usage = norm_usage(&data, &["prompt_eval_count"], &["eval_count"]);
        let text = data
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| ModelError::BadResponse("no message in Ollama response".into()))?
            .to_string();
        Ok((text, usage))
    }
}

/// Deterministic offline response with structured reasoning.
///
/// This is NOT a model call. It restates the task, shows the reasoning
/// steps a specialist would take, and suggests next steps, so demos,
/// tests, and the API server all work with no API keys and no network.
pub fn stub_chat(model_id: &str, messages: &[ChatMessage]) -> String {
    let mut task = String::new();
    for message in messages.iter().rev() {
        if message.role == "user" {
            task = message.content.clone();
            break;
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(task.as_bytes());
    let digest = hasher.finalize();
    let variant = (digest[0] as usize) % 3;

    let openers = [
        "Breaking this down methodically.",
        "Working through this step by step.",
        "Approaching this in a structured way.",
    ];
    let mut focus_words: Vec<String> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for word in task.split_whitespace() {
        let cleaned = word
            .trim_matches(|c: char| ".,!?".contains(c))
            .to_lowercase();
        if cleaned.len() > 5 && !seen.contains_key(&cleaned) {
            seen.insert(cleaned.clone(), ());
            focus_words.push(cleaned);
        }
    }
    focus_words.sort();
    focus_words.truncate(5);
    let focus = focus_words.join(", ");

    let lines = [
        "[offline stub: no model API key configured, structured reasoning only]".to_string(),
        String::new(),
        format!("Model requested: {model_id}"),
        String::new(),
        "Task restated:".to_string(),
        task.trim().to_string(),
        String::new(),
        format!("Reasoning: {}", openers[variant]),
        format!(
            "1. Identify the core ask and the key terms involved ({}).",
            if focus.is_empty() {
                "general request".to_string()
            } else {
                focus
            }
        ),
        "2. Apply domain expertise to separate facts, assumptions, and unknowns.".to_string(),
        "3. Draft a response that is specific, checkable, and scoped to the request.".to_string(),
        "4. Note what would need verification with live data or tools.".to_string(),
        String::new(),
        "Suggested next steps:".to_string(),
        "- Refine the request with concrete constraints or examples.".to_string(),
        "- Run the relevant builtin tools to ground the answer in data.".to_string(),
        "- Escalate to a specialist agent if the task crosses departments.".to_string(),
        String::new(),
        "Set a model API key (OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY) ".to_string()
            + "or run Ollama locally to get a real model response instead of this stub.",
    ];
    lines.join("\n")
}

/// A scripted client for tests and evals: plays canned replies in
/// order, repeating the last one when the queue runs out.
pub struct ScriptedClient {
    replies: Mutex<Vec<String>>,
    calls: Mutex<usize>,
    seen: Mutex<Vec<Vec<ChatMessage>>>,
    provider: String,
}

impl ScriptedClient {
    pub fn new(replies: Vec<String>) -> Self {
        ScriptedClient {
            replies: Mutex::new(replies),
            calls: Mutex::new(0),
            seen: Mutex::new(Vec::new()),
            provider: "scripted".to_string(),
        }
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn seen(&self) -> Vec<Vec<ChatMessage>> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl ModelClient for ScriptedClient {
    fn chat(&self, _model_id: &str, messages: &[ChatMessage]) -> Result<String, ModelError> {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        *calls += 1;
        let index = *calls;
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(messages.to_vec());
        }
        let replies = self.replies.lock().unwrap_or_else(|e| e.into_inner());
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
        self.provider.clone()
    }

    fn last_usage(&self) -> Usage {
        Usage::default()
    }
}

/// A client that fails every call, for tests of degraded paths.
pub struct FailingClient;

impl ModelClient for FailingClient {
    fn chat(&self, _model_id: &str, _messages: &[ChatMessage]) -> Result<String, ModelError> {
        Err(ModelError::Provider("provider down".to_string()))
    }

    fn provider_for(&self, _model_id: &str) -> String {
        "stub".to_string()
    }

    fn last_usage(&self) -> Usage {
        Usage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_chat_is_deterministic_and_labeled() {
        let messages = vec![ChatMessage::new("user", "What is 6 times 7?")];
        let first = stub_chat("stub", &messages);
        let second = stub_chat("stub", &messages);
        assert_eq!(first, second);
        assert!(first.contains("[offline stub"));
        assert!(first.contains("What is 6 times 7?"));
    }

    #[test]
    fn team_key_wins_over_platform_key_then_stub() {
        let _guard = crate::test_support::lock_env();
        std::env::remove_var("OPENAI_API_KEY");
        let client = RealClient::new("gpt-4o");
        assert_eq!(client.resolve_provider("gpt-4o"), "stub");
        let mut team_keys = HashMap::new();
        team_keys.insert("openai".to_string(), "sk-team-key".to_string());
        with_request_provider_keys(&team_keys, || {
            assert_eq!(client.resolve_provider("gpt-4o"), "openai");
            assert_eq!(key_for_provider("openai").as_deref(), Some("sk-team-key"));
        });
        std::env::set_var("OPENAI_API_KEY", "sk-platform-key");
        assert_eq!(client.resolve_provider("gpt-4o"), "openai");
        assert_eq!(
            key_for_provider("openai").as_deref(),
            Some("sk-platform-key")
        );
        with_request_provider_keys(&team_keys, || {
            assert_eq!(key_for_provider("openai").as_deref(), Some("sk-team-key"));
        });
        std::env::remove_var("OPENAI_API_KEY");
    }

    #[test]
    fn infer_provider_known_prefixes() {
        assert_eq!(infer_provider("gpt-4o"), "openai");
        assert_eq!(infer_provider("claude-sonnet-4-5"), "anthropic");
        assert_eq!(infer_provider("gemini-2.5-pro"), "google");
        assert_eq!(infer_provider("something-else"), "stub");
        assert_eq!(infer_provider("stub"), "stub");
    }
}
