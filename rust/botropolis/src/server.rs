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
//!     POST /signup           Register a team, returns a session token
//!     POST /signin           Sign in, returns a session token
//!     POST /teams/api-keys   Create a team API key (shown once)
//!     GET  /teams/api-keys   List a team's API keys (metadata only)
//!     DELETE /teams/api-keys/{id} Revoke one of a team's API keys
//!     POST /teams/provider-keys Store a team provider key (BYOK)
//!     GET  /teams/provider-keys List stored provider key names only
//!     DELETE /teams/provider-keys/{provider} Delete a stored key
//!     GET  /teams/usage       A team's usage and quota for the period
//!     GET  /                 Web UI (static files under /static)
//!
//! The web UI lives in botropolis/web/ and needs no build step. Agent
//! runs are blocking work, so they run on the blocking thread pool
//! while the server stays responsive. Mirrors `botropolis/server.py`.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tower_http::services::ServeDir;

use crate::agent::RunOpts;
use crate::memory::{default_memory_store, set_memory_store};
use crate::orchestrator::{history_context, ChatTurn, CEO, MAX_TEAM_ROUNDS};
use crate::platform::{ApiKeyMeta, PlatformDb, Team, SESSION_TTL_SECS};
use crate::quotas::{quota_error, quota_status};
use crate::registry::AgentRegistry;
use crate::schemas::{new_run_id, AgentResult, Contract, Task};
use crate::tenancy::{cookie_value, hosted_mode, SESSION_COOKIE, TENANT_KEY_HEADER};
use crate::tenancy::{TenantContext, DEFAULT_TENANT};
use crate::tools::builtin::notebook_path;
use crate::tools::consult::set_consult_registry;
use crate::usage::{get_summary_for, get_summary_for_period, log_invocation_for};

/// Shared server state: the registry, the hosted flag, the platform
/// database, and one CEO per tenant, built lazily so each tenant
/// gets its own memory, ledger, and run state.
#[derive(Debug)]
pub struct AppState {
    pub registry: Arc<AgentRegistry>,
    pub hosted: bool,
    pub platform: PlatformDb,
    tenants: Mutex<HashMap<String, CEO>>,
}

impl AppState {
    /// Build the default state: load the registry, wire the shared
    /// memory store and the consult registry, and open the platform
    /// database. Hosted mode comes from `BOTROPOLIS_HOSTED`.
    pub fn load() -> Result<Self, String> {
        Self::load_with(hosted_mode())
    }

    /// Build state with an explicit hosted flag. Tests use this to
    /// run the same app in hosted mode without touching env.
    pub fn load_with(hosted: bool) -> Result<Self, String> {
        let registry = Arc::new(AgentRegistry::load().map_err(|e| e.to_string())?);
        let store = default_memory_store();
        set_memory_store(Some(store.clone()));
        set_consult_registry(Some(Arc::clone(&registry)));
        let platform = PlatformDb::open_default()?;
        Ok(AppState {
            registry,
            hosted,
            platform,
            tenants: Mutex::new(HashMap::new()),
        })
    }

    /// The CEO for one tenant, built on first use and cached after.
    pub fn ceo_for(&self, tenant_id: &str) -> Result<CEO, String> {
        let mut tenants = self.tenants.lock().map_err(|e| e.to_string())?;
        if let Some(ceo) = tenants.get(tenant_id) {
            return Ok(ceo.clone());
        }
        let ceo = CEO::for_tenant(Some(Arc::clone(&self.registry)), tenant_id)?;
        tenants.insert(tenant_id.to_string(), ceo.clone());
        Ok(ceo)
    }

    /// The requesting team's decrypted provider keys, for the model
    /// client to prefer over the platform keys. Empty when the team
    /// has none stored, or the master key is not set.
    pub fn provider_keys_for(&self, team_id: &str) -> HashMap<String, String> {
        if team_id == DEFAULT_TENANT {
            return HashMap::new();
        }
        self.platform.provider_keys(team_id)
    }
}

/// Resolve the tenant for a request from its headers, through the
/// platform database (so a successful key lookup also refreshes
/// `last_used_at`). In hosted mode a request with no tenant
/// credential is rejected with 401; in single-tenant mode it falls
/// back to the default tenant.
fn tenant_from(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<TenantContext, (StatusCode, Json<Value>)> {
    let api_key = headers
        .get(TENANT_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if let Some(key) = api_key {
        return match state.platform.team_from_key(key) {
            Some(team) => Ok(TenantContext::team(&team.id)),
            None => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"detail": "unknown or revoked team key"})),
            )),
        };
    }
    if let Some(cookie) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = cookie_value(cookie, SESSION_COOKIE) {
            if !token.is_empty() {
                return match state.platform.team_from_session(&token) {
                    Some(team) => Ok(TenantContext::team(&team.id)),
                    None => Err((
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"detail": "unknown or expired session"})),
                    )),
                };
            }
        }
    }
    if state.hosted {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"detail": "a team key or session is required"})),
        ));
    }
    Ok(TenantContext::default_tenant())
}

/// The signed-in team for a request: a session cookie first, then
/// a team API key. Either proves the team.
fn auth_team(state: &AppState, headers: &HeaderMap) -> Result<Team, (StatusCode, Json<Value>)> {
    if let Some(cookie) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = cookie_value(cookie, SESSION_COOKIE) {
            if !token.is_empty() {
                if let Some(team) = state.platform.team_from_session(&token) {
                    return Ok(team);
                }
            }
        }
    }
    if let Some(key) = headers
        .get(TENANT_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        if let Some(team) = state.platform.team_from_key(key) {
            return Ok(team);
        }
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({"detail": "a valid session or team key is required"})),
    ))
}

/// The tenant id for state reads that do not run agents: the
/// notebook and the usage summary are per tenant.
fn tenant_notebook_path(tenant_id: &str) -> std::path::PathBuf {
    if tenant_id == DEFAULT_TENANT {
        notebook_path()
    } else {
        crate::paths::tenant_notebook_path(tenant_id)
    }
}

/// Reject a run when the tenant's team has used up its quota for
/// the current month. The default tenant has no team, so local dev
/// is never quota-limited. Reads are never blocked by this.
fn check_quota(state: &AppState, tenant_id: &str) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(team) = state.platform.team_by_id(tenant_id) else {
        return Ok(());
    };
    let status = quota_status(tenant_id, &team.plan);
    if status.exceeded {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({"detail": quota_error(&status), "quota": status})),
        ));
    }
    Ok(())
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

/// Write one row to the tenant's usage log. Never raises.
fn record_usage(tenant_id: &str, result: &AgentResult) {
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
    log_invocation_for(
        tenant_id,
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
    headers: HeaderMap,
    Json(body): Json<AskRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant = tenant_from(&state, &headers)?;
    validate_ask(&body).map_err(bad_request)?;
    check_quota(&state, &tenant.tenant_id)?;
    let history = body.history.clone();
    let contract = body.contract.as_ref().map(|c| c.to_contract());
    let needs_approval = body.needs_approval.clone();
    let approved = body.approved;
    let ceo = state.ceo_for(&tenant.tenant_id).map_err(bad_request)?;
    let request = body.request.clone();
    let tenant_id = tenant.tenant_id.clone();
    let provider_keys = state.provider_keys_for(&tenant.tenant_id);
    let report = tokio::task::spawn_blocking(move || {
        crate::tenancy::with_tenant(&tenant_id, || {
            crate::models::with_request_provider_keys(&provider_keys, || {
                ceo.handle(
                    &request,
                    Some(&history),
                    contract,
                    &needs_approval,
                    approved,
                )
            })
        })
    })
    .await
    .map_err(|e| bad_request(format!("run failed: {e}")))?
    .map_err(bad_request)?;
    for result in &report.results {
        record_usage(&tenant.tenant_id, result);
    }
    Ok(Json(serde_json::to_value(&report).unwrap_or(Value::Null)))
}

async fn ask_agent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<AskRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant = tenant_from(&state, &headers)?;
    validate_ask(&body).map_err(bad_request)?;
    let agent = state
        .registry
        .get(&name)
        .map_err(|_| not_found(format!("Unknown agent: {name}")))?;
    let history = body.history.clone();
    let request = body.request.clone();
    let ceo = state.ceo_for(&tenant.tenant_id).map_err(bad_request)?;
    let tenant_id = tenant.tenant_id.clone();
    let provider_keys = state.provider_keys_for(&tenant.tenant_id);
    let result = tokio::task::spawn_blocking(move || {
        crate::tenancy::with_tenant(&tenant_id, || {
            crate::models::with_request_provider_keys(&provider_keys, || {
                agent.run(
                    agent_task(&agent, &request, &history),
                    RunOpts {
                        ledger: Some(ceo.ledger.clone()),
                        run_id: Some(new_run_id()),
                        store: ceo.run_store.clone(),
                        ..Default::default()
                    },
                )
            })
        })
    })
    .await
    .map_err(|e| bad_request(format!("run failed: {e}")))?;
    record_usage(&tenant.tenant_id, &result);
    Ok(Json(serde_json::to_value(&result).unwrap_or(Value::Null)))
}

async fn team(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TeamRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant = tenant_from(&state, &headers)?;
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
    check_quota(&state, &tenant.tenant_id)?;
    let ceo = state.ceo_for(&tenant.tenant_id).map_err(bad_request)?;
    let request = body.request.clone();
    let agents = body.agents.clone();
    let rounds = body.rounds;
    let tenant_id = tenant.tenant_id.clone();
    let provider_keys = state.provider_keys_for(&tenant.tenant_id);
    let report = tokio::task::spawn_blocking(move || {
        crate::tenancy::with_tenant(&tenant_id, || {
            crate::models::with_request_provider_keys(&provider_keys, || {
                ceo.team(&request, &agents, rounds, None, &[], false)
            })
        })
    })
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
        record_usage(&tenant.tenant_id, &round.result);
    }
    Ok(Json(serde_json::to_value(&report).unwrap_or(Value::Null)))
}

async fn analytics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant = tenant_from(&state, &headers)?;
    Ok(Json(get_summary_for(&tenant.tenant_id)))
}

async fn notebook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant = tenant_from(&state, &headers)?;
    let content = std::fs::read_to_string(tenant_notebook_path(&tenant.tenant_id))
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(Json(json!({"content": content})))
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
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<
    Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<Value>),
> {
    let tenant = tenant_from(&state, &headers)?;
    if query.request.trim().is_empty() {
        return Err(bad_request("request must not be empty".to_string()));
    }
    let agent = state
        .registry
        .get(&name)
        .map_err(|_| not_found(format!("Unknown agent: {name}")))?;
    let history = parse_history_param(&query.history);
    let request = query.request.clone();
    let ceo = state.ceo_for(&tenant.tenant_id).map_err(bad_request)?;
    let tenant_id = tenant.tenant_id.clone();
    let provider_keys = state.provider_keys_for(&tenant.tenant_id);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<(Option<String>, Value)>();
    tokio::task::spawn_blocking(move || {
        crate::tenancy::with_tenant(&tenant_id, || {
            crate::models::with_request_provider_keys(&provider_keys, || {
                let sender = tx.clone();
                let opts = RunOpts {
                    ledger: Some(ceo.ledger.clone()),
                    run_id: Some(new_run_id()),
                    store: ceo.run_store.clone(),
                    on_event: Some(Box::new(move |kind: &str, payload: Value| {
                        let _ = sender.send((Some(kind.to_string()), payload));
                    })),
                    ..Default::default()
                };
                let task = agent_task(&agent, &request, &history);
                let result = agent.run(task, opts);
                record_usage(&tenant_id, &result);
                let _ = tx.send((
                    "result".to_string().into(),
                    serde_json::to_value(&result).unwrap_or(Value::Null),
                ));
                let _ = tx.send(("done".to_string().into(), json!({})));
            })
        });
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

/// Body for POST /signup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    pub name: String,
}

/// Body for POST /signin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigninRequest {
    pub email: String,
    pub password: String,
}

/// Body for POST /teams/api-keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKeyRequest {
    #[serde(default)]
    pub name: String,
}

/// A `Set-Cookie` header for a fresh session token. HttpOnly, so
/// page scripts cannot read it.
fn session_cookie(token: &str) -> (axum::http::HeaderName, axum::http::HeaderValue) {
    let value = format!(
        "{SESSION_COOKIE}={token}; HttpOnly; Path=/; SameSite=Lax; Max-Age={}",
        SESSION_TTL_SECS as u64
    );
    (
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_str(&value)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
    )
}

async fn signup(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SignupRequest>,
) -> Result<(StatusCode, HeaderMap, Json<Value>), (StatusCode, Json<Value>)> {
    let team = state
        .platform
        .create_team(&body.name, &body.email, &body.password)
        .map_err(bad_request)?;
    let token = state
        .platform
        .create_session(&team.id)
        .map_err(bad_request)?;
    let mut headers = HeaderMap::new();
    let (name, value) = session_cookie(&token);
    headers.insert(name, value);
    Ok((
        StatusCode::CREATED,
        headers,
        Json(json!({"team": team, "session_token": token})),
    ))
}

async fn signin(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SigninRequest>,
) -> Result<(StatusCode, HeaderMap, Json<Value>), (StatusCode, Json<Value>)> {
    let team = state
        .platform
        .verify_signin(&body.email, &body.password)
        .map_err(|e| (StatusCode::UNAUTHORIZED, Json(json!({"detail": e}))))?;
    let token = state
        .platform
        .create_session(&team.id)
        .map_err(bad_request)?;
    let mut headers = HeaderMap::new();
    let (name, value) = session_cookie(&token);
    headers.insert(name, value);
    Ok((
        StatusCode::OK,
        headers,
        Json(json!({"team": team, "session_token": token})),
    ))
}

async fn create_api_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    let (meta, key) = state
        .platform
        .create_api_key(&team.id, &body.name)
        .map_err(bad_request)?;
    Ok(Json(json!({
        "id": meta.id,
        "name": meta.name,
        "key": key,
        "created_at": meta.created_at,
    })))
}

async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    let keys: Vec<ApiKeyMeta> = state.platform.list_api_keys(&team.id);
    Ok(Json(json!({"api_keys": keys})))
}

async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    let revoked = state
        .platform
        .revoke_api_key(&team.id, &key_id)
        .map_err(bad_request)?;
    if !revoked {
        return Err(not_found(format!("Unknown API key: {key_id}")));
    }
    Ok(Json(json!({"revoked": true})))
}

/// Body for POST /teams/provider-keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreProviderKeyRequest {
    pub provider: String,
    pub key: String,
}

async fn store_provider_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<StoreProviderKeyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    if crate::crypto::master_key().is_err() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({"detail": "provider keys need a platform master key: set BOTROPOLIS_MASTER_KEY on the server"}),
            ),
        ));
    }
    let provider = body.provider.trim().to_lowercase();
    state
        .platform
        .store_provider_key(&team.id, &provider, &body.key)
        .map_err(bad_request)?;
    Ok(Json(json!({"provider": provider, "stored": true})))
}

async fn list_provider_keys(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    let providers = state.platform.list_provider_keys(&team.id);
    Ok(Json(json!({"providers": providers})))
}

async fn delete_provider_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    let deleted = state
        .platform
        .delete_provider_key(&team.id, &provider)
        .map_err(bad_request)?;
    if !deleted {
        return Err(not_found(format!("No stored key for provider: {provider}")));
    }
    Ok(Json(json!({"deleted": true})))
}

/// A team's usage for the current period, with its quota standing.
async fn team_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let team = auth_team(&state, &headers)?;
    let status = quota_status(&team.id, &team.plan);
    let start = crate::quotas::period_start_secs(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
    );
    let summary = get_summary_for_period(&team.id, start);
    Ok(Json(json!({
        "team_id": team.id,
        "plan": team.plan,
        "period": status.period,
        "requests": status.requests_used,
        "tokens": status.tokens_used,
        "tool_calls": summary["totals"]["tool_calls"],
        "agents": summary["agents"],
        "quota": status,
    })))
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
        .route("/signup", post(signup))
        .route("/signin", post(signin))
        .route("/teams/api-keys", post(create_api_key).get(list_api_keys))
        .route("/teams/api-keys/{id}", delete(revoke_api_key))
        .route(
            "/teams/provider-keys",
            post(store_provider_key).get(list_provider_keys),
        )
        .route(
            "/teams/provider-keys/{provider}",
            delete(delete_provider_key),
        )
        .route("/teams/usage", get(team_usage))
        .nest_service("/static", ServeDir::new(web_dir))
        .with_state(state)
}

/// Start the web server on 127.0.0.1:port. Blocks until it stops.
/// Hosted mode comes from `BOTROPOLIS_HOSTED`.
pub async fn serve(port: u16) -> Result<(), String> {
    serve_hosted(port, crate::tenancy::hosted_mode()).await
}

/// Start the web server with an explicit hosted flag. In hosted
/// mode, requests without a team key or session are rejected.
pub async fn serve_hosted(port: u16, hosted: bool) -> Result<(), String> {
    let state = Arc::new(AppState::load_with(hosted)?);
    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| e.to_string())?;
    println!("Botropolis serving on http://127.0.0.1:{port}");
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}
