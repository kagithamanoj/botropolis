//! HTTP API for Botropolis.
//!
//! Endpoints:
//!     POST /ask              Route a request through the CEO orchestrator
//!     GET  /agents           List every registered agent
//!     GET  /agents/{name}    Show one agent's details
//!     POST /agents/{name}/ask Chat with one agent directly (war room)
//!     GET  /agents/{name}/ask/stream Stream one agent's tool calls (SSE)
//!     POST /team           Run a team of agents in collaboration rounds
//!     GET  /analytics        Per-agent usage totals (calls, latency, tokens)
//!     GET  /departments      List departments and their headcounts
//!     GET  /health           Liveness check
//!     GET  /                 Web UI (static files under /static)
//!
//! The web UI lives in botropolis/web/ and needs no build step. Agent
//! runs are blocking work, so they run on the blocking thread pool
//! while the server stays responsive. Mirrors `botropolis/server.py`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tower_http::services::ServeDir;

use crate::agent::RunOpts;
use crate::memory::{default_memory_store, set_memory_store};
use crate::orchestrator::{history_context, ChatTurn, CEO, MAX_TEAM_ROUNDS};
use crate::registry::AgentRegistry;
use crate::schemas::{AgentResult, Contract, Task};
use crate::tools::builtin::notebook_path;
use crate::tools::consult::set_consult_registry;
use crate::usage::{get_summary, log_invocation};

/// Shared server state: the registry and the CEO that routes requests.
#[derive(Debug, Clone)]
pub struct AppState {
    pub registry: Arc<AgentRegistry>,
    pub ceo: CEO,
}

impl AppState {
    /// Build the default state: load the registry, wire the shared
    /// memory store and the consult registry, and build the CEO.
    pub fn load() -> Result<Self, String> {
        let registry = Arc::new(AgentRegistry::load().map_err(|e| e.to_string())?);
        let store = default_memory_store();
        set_memory_store(Some(store.clone()));
        set_consult_registry(Some(Arc::clone(&registry)));
        let ceo = CEO::new(Some(Arc::clone(&registry)), None, Some(store))?;
        Ok(AppState { registry, ceo })
    }
}

/// One machine check on a contract: a shell command and the exit
/// code that counts as passing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractCheckIn {
    pub command: String,
    #[serde(default)]
    pub expected_exit_code: i32,
    #[serde(default)]
    pub name: String,
}

/// A task contract: the agreement the run is held to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractIn {
    pub goal: String,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub checks: Vec<ContractCheckIn>,
}

impl ContractIn {
    fn to_contract(&self) -> Contract {
        Contract {
            goal: self.goal.clone(),
            scope: self.scope.clone(),
            exclusions: self.exclusions.clone(),
            done_when: self.done_when.clone(),
            checks: self
                .checks
                .iter()
                .map(|c| crate::schemas::ContractCheck {
                    command: c.command.clone(),
                    expected_exit_code: c.expected_exit_code,
                    name: c.name.clone(),
                })
                .collect(),
        }
    }
}

/// Body for POST /ask.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskRequest {
    pub request: String,
    #[serde(default)]
    pub history: Vec<ChatTurn>,
    #[serde(default)]
    pub contract: Option<ContractIn>,
    #[serde(default)]
    pub needs_approval: Vec<String>,
    #[serde(default)]
    pub approved: bool,
}

/// Body for POST /team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRequest {
    pub request: String,
    pub agents: Vec<String>,
    #[serde(default = "default_rounds")]
    pub rounds: u32,
}

fn default_rounds() -> u32 {
    2
}

fn validate_history(history: &[ChatTurn]) -> Result<(), String> {
    if history.len() > 40 {
        return Err("history holds at most 40 turns".to_string());
    }
    for turn in history {
        if turn.role != "user" && turn.role != "assistant" {
            return Err(format!("bad history role: {}", turn.role));
        }
        if turn.content.is_empty() || turn.content.chars().count() > 4000 {
            return Err("history content must be 1 to 4000 characters".to_string());
        }
    }
    Ok(())
}

fn validate_ask(body: &AskRequest) -> Result<(), String> {
    if body.request.trim().is_empty() {
        return Err("request must not be empty".to_string());
    }
    validate_history(&body.history)?;
    if let Some(contract) = &body.contract {
        if contract.goal.trim().is_empty() {
            return Err("contract goal must not be empty".to_string());
        }
        for check in &contract.checks {
            if check.command.trim().is_empty() {
                return Err("contract check command must not be empty".to_string());
            }
        }
    }
    Ok(())
}

fn bad_request(message: String) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"detail": message})))
}

fn not_found(message: String) -> (StatusCode, Json<Value>) {
    (StatusCode::NOT_FOUND, Json(json!({"detail": message})))
}

/// Write one row to the usage log. Never raises.
fn record_usage(result: &AgentResult) {
    let meta = &result.metadata;
    let model = meta
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let provider = meta
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("stub")
        .to_string();
    let tokens_in = meta.get("tokens_in").and_then(|v| v.as_u64()).unwrap_or(0);
    let tokens_out = meta.get("tokens_out").and_then(|v| v.as_u64()).unwrap_or(0);
    let tool_names: Vec<String> = result.tool_calls.iter().map(|c| c.tool.clone()).collect();
    log_invocation(
        &result.agent_name,
        &result.department,
        &model,
        &provider,
        result.elapsed_seconds * 1000.0,
        tokens_in,
        tokens_out,
        result.success,
        result.tool_calls.len(),
        &tool_names,
    );
}

/// Build a Task for a direct agent chat, with history as context.
fn agent_task(agent: &crate::agent::Agent, request_text: &str, history: &[ChatTurn]) -> Task {
    let mut task = Task::new(request_text);
    task.department = Some(agent.department.clone());
    task.agent_name = Some(agent.name.clone());
    task.context = history_context(Some(history));
    task
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"status": "ok", "agents": state.registry.len()}))
}

async fn index() -> impl IntoResponse {
    let path = crate::paths::web_dir().join("index.html");
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/html")],
            bytes,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({"detail": "web UI not found"})),
        )
            .into_response(),
    }
}

async fn ask(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AskRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    validate_ask(&body).map_err(bad_request)?;
    let history = body.history.clone();
    let contract = body.contract.as_ref().map(|c| c.to_contract());
    let needs_approval = body.needs_approval.clone();
    let approved = body.approved;
    let ceo = state.ceo.clone();
    let request = body.request.clone();
    let report = tokio::task::spawn_blocking(move || {
        ceo.handle(
            &request,
            Some(&history),
            contract,
            &needs_approval,
            approved,
        )
    })
    .await
    .map_err(|e| bad_request(format!("run failed: {e}")))?
    .map_err(bad_request)?;
    for result in &report.results {
        record_usage(result);
    }
    Ok(Json(serde_json::to_value(&report).unwrap_or(Value::Null)))
}

async fn ask_agent(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<AskRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    validate_ask(&body).map_err(bad_request)?;
    let agent = state
        .registry
        .get(&name)
        .map_err(|_| not_found(format!("Unknown agent: {name}")))?;
    let history = body.history.clone();
    let request = body.request.clone();
    let result = tokio::task::spawn_blocking(move || {
        agent.run(agent_task(&agent, &request, &history), RunOpts::default())
    })
    .await
    .map_err(|e| bad_request(format!("run failed: {e}")))?;
    record_usage(&result);
    Ok(Json(serde_json::to_value(&result).unwrap_or(Value::Null)))
}

async fn team(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TeamRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.request.trim().is_empty() {
        return Err(bad_request("request must not be empty".to_string()));
    }
    if body.agents.is_empty() {
        return Err(bad_request("team needs at least one agent".to_string()));
    }
    if body.rounds < 1 || body.rounds > MAX_TEAM_ROUNDS {
        return Err(bad_request(format!(
            "rounds must be between 1 and {MAX_TEAM_ROUNDS}"
        )));
    }
    let ceo = state.ceo.clone();
    let request = body.request.clone();
    let agents = body.agents.clone();
    let rounds = body.rounds;
    let report =
        tokio::task::spawn_blocking(move || ceo.team(&request, &agents, rounds, None, &[], false))
            .await
            .map_err(|e| bad_request(format!("run failed: {e}")))?
            .map_err(|e| {
                if e.starts_with("Unknown agent") {
                    not_found(e)
                } else {
                    bad_request(e)
                }
            })?;
    for round in &report.rounds {
        record_usage(&round.result);
    }
    Ok(Json(serde_json::to_value(&report).unwrap_or(Value::Null)))
}

async fn analytics() -> Json<Value> {
    Json(get_summary())
}

async fn notebook() -> Json<Value> {
    let content = std::fs::read_to_string(notebook_path())
        .unwrap_or_default()
        .trim()
        .to_string();
    Json(json!({"content": content}))
}

async fn list_agents(State(state): State<Arc<AppState>>) -> Json<Value> {
    let agents: Vec<Value> = state
        .registry
        .list_all()
        .iter()
        .map(|a| a.describe())
        .collect();
    Json(json!({"agents": agents}))
}

async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let agent = state
        .registry
        .get(&name)
        .map_err(|_| not_found(format!("Unknown agent: {name}")))?;
    Ok(Json(agent.describe()))
}

async fn list_departments(State(state): State<Arc<AppState>>) -> Json<Value> {
    let departments: Vec<Value> = state
        .registry
        .departments()
        .iter()
        .map(|dept| {
            json!({
                "name": dept,
                "agents": state.registry.by_department(dept).iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({"departments": departments}))
}

/// Query params for the SSE stream: the request plus JSON history.
#[derive(Debug, Deserialize)]
struct StreamQuery {
    request: String,
    #[serde(default = "default_history_json")]
    history: String,
}

fn default_history_json() -> String {
    "[]".to_string()
}

/// Parse the JSON-encoded history query param. Never raises.
fn parse_history_param(raw: &str) -> Vec<ChatTurn> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let mut clean = Vec::new();
    if let Some(turns) = parsed.as_array() {
        for turn in turns.iter().rev().take(40).rev() {
            let role = turn.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = turn.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if (role == "user" || role == "assistant") && !content.is_empty() {
                clean.push(ChatTurn {
                    role: role.to_string(),
                    content: content.chars().take(4000).collect(),
                });
            }
        }
    }
    clean
}

async fn ask_agent_stream(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<
    Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<Value>),
> {
    if query.request.trim().is_empty() {
        return Err(bad_request("request must not be empty".to_string()));
    }
    let agent = state
        .registry
        .get(&name)
        .map_err(|_| not_found(format!("Unknown agent: {name}")))?;
    let history = parse_history_param(&query.history);
    let request = query.request.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<(Option<String>, Value)>();
    tokio::task::spawn_blocking(move || {
        let sender = tx.clone();
        let opts = RunOpts {
            on_event: Some(Box::new(move |kind: &str, payload: Value| {
                let _ = sender.send((Some(kind.to_string()), payload));
            })),
            ..Default::default()
        };
        let task = agent_task(&agent, &request, &history);
        let result = agent.run(task, opts);
        record_usage(&result);
        let _ = tx.send((
            "result".to_string().into(),
            serde_json::to_value(&result).unwrap_or(Value::Null),
        ));
        let _ = tx.send(("done".to_string().into(), json!({})));
    });
    let stream = UnboundedReceiverStream::new(rx).map(|(kind, payload)| {
        let kind = kind.unwrap_or_else(|| "done".to_string());
        let data = serde_json::to_string(&payload).unwrap_or_default();
        Ok(Event::default().event(kind).data(data))
    });
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// Build the router for one loaded state.
pub fn build_app(state: Arc<AppState>) -> Router {
    let web_dir = crate::paths::web_dir();
    Router::new()
        .route("/health", get(health))
        .route("/", get(index))
        .route("/ask", post(ask))
        .route("/agents", get(list_agents))
        .route("/agents/{name}", get(get_agent))
        .route("/agents/{name}/ask", post(ask_agent))
        .route("/agents/{name}/ask/stream", get(ask_agent_stream))
        .route("/team", post(team))
        .route("/analytics", get(analytics))
        .route("/notebook", get(notebook))
        .route("/departments", get(list_departments))
        .nest_service("/static", ServeDir::new(web_dir))
        .with_state(state)
}

/// Start the web server on 127.0.0.1:port. Blocks until it stops.
pub async fn serve(port: u16) -> Result<(), String> {
    let state = Arc::new(AppState::load()?);
    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| e.to_string())?;
    println!("Botropolis serving on http://127.0.0.1:{port}");
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}
