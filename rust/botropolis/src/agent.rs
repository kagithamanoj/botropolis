//! The Agent: every Botropolis employee, loaded from a YAML spec.
//!
//! An agent is defined by a YAML spec file (see `botropolis/agents/`)
//! and is executed through a ModelClient. When no model API key is
//! configured, the agent runs against the deterministic offline stub so
//! demos and tests work anywhere. Mirrors `botropolis/core/agent.py`.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use thiserror::Error;

use crate::ledger::Ledger;
use crate::models::{ChatMessage, ModelClient, RealClient};
use crate::output_schemas::{
    format_instruction, parse_structured_output, resolve_output_schema, AgentOutput,
};
use crate::runstate::RunStore;
use crate::schemas::{AgentResult, Task};
use crate::tool_loop::{run_tool_loop, EventCallback, LoopOpts, StateHook};
use crate::tools::{get_tool_def, known_tool};

pub const DEFAULT_MAX_STEPS: u32 = 8;

pub const REQUIRED_SPEC_FIELDS: [&str; 6] = [
    "name",
    "title",
    "department",
    "specialty",
    "model",
    "system_prompt",
];

/// The eleven departments in the org chart.
pub const KNOWN_DEPARTMENTS: [&str; 11] = [
    "research",
    "health",
    "finance",
    "code",
    "data",
    "legal",
    "marketing",
    "ops",
    "security",
    "support",
    "sales",
];

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("{0}")]
    Load(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
}

/// A specialist employee of Botropolis, loaded from a YAML spec.
pub struct Agent {
    pub spec_path: PathBuf,
    pub name: String,
    pub title: String,
    pub role: String,
    pub department: String,
    pub specialty: String,
    pub model: String,
    pub system_prompt: String,
    pub toolkit: Vec<String>,
    pub max_steps: u32,
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<f64>,
    pub example_tasks: Vec<serde_json::Value>,
    pub output_schema_name: &'static str,
    client: RwLock<Arc<dyn ModelClient>>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("name", &self.name)
            .field("department", &self.department)
            .field("model", &self.model)
            .field("toolkit", &self.toolkit)
            .finish()
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawSpec {
    name: Option<String>,
    title: Option<String>,
    department: Option<String>,
    specialty: Option<String>,
    model: Option<String>,
    system_prompt: Option<String>,
    toolkit: Option<Vec<String>>,
    max_steps: Option<u32>,
    token_budget: Option<u64>,
    time_budget_seconds: Option<f64>,
    example_tasks: Option<Vec<serde_yaml::Value>>,
    output_schema: Option<String>,
}

impl Agent {
    /// Load an agent from a spec file.
    pub fn load(
        spec_path: impl AsRef<Path>,
        client: Option<Arc<dyn ModelClient>>,
    ) -> Result<Agent, AgentError> {
        let spec_path = spec_path.as_ref().to_path_buf();
        let text = std::fs::read_to_string(&spec_path)?;
        let spec: RawSpec = serde_yaml::from_str(&text)?;
        let missing: Vec<&str> = REQUIRED_SPEC_FIELDS
            .iter()
            .copied()
            .filter(|field| match *field {
                "name" => spec.name.is_none(),
                "title" => spec.title.is_none(),
                "department" => spec.department.is_none(),
                "specialty" => spec.specialty.is_none(),
                "model" => spec.model.is_none(),
                "system_prompt" => spec.system_prompt.is_none(),
                _ => false,
            })
            .collect();
        if !missing.is_empty() {
            return Err(AgentError::Load(format!(
                "{}: missing fields {:?}",
                spec_path.display(),
                missing
            )));
        }
        let name = spec.name.unwrap();
        let department = spec.department.unwrap();
        if !KNOWN_DEPARTMENTS.contains(&department.as_str()) {
            return Err(AgentError::Load(format!(
                "{}: unknown department {department:?}. Known: {:?}",
                spec_path.display(),
                KNOWN_DEPARTMENTS
            )));
        }
        let toolkit = spec.toolkit.unwrap_or_default();
        let unknown: Vec<&str> = toolkit
            .iter()
            .map(|t| t.as_str())
            .filter(|t| !known_tool(t))
            .collect();
        if !unknown.is_empty() {
            return Err(AgentError::Load(format!(
                "{}: unknown tools in toolkit: {:?}. Available: {:?}",
                spec_path.display(),
                unknown,
                crate::tools::tool_names()
            )));
        }
        let output_schema_name = resolve_output_schema(spec.output_schema.as_deref(), &department)
            .map_err(|e| AgentError::Load(format!("{}: {e}", spec_path.display())))?;
        let client = client.unwrap_or_else(|| Arc::new(RealClient::default()));
        Ok(Agent {
            spec_path,
            title: spec.title.clone().unwrap(),
            role: spec.title.unwrap(),
            name,
            department,
            specialty: spec.specialty.unwrap(),
            model: spec.model.unwrap(),
            system_prompt: spec.system_prompt.unwrap(),
            toolkit,
            max_steps: spec.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
            token_budget: spec.token_budget,
            time_budget_seconds: spec.time_budget_seconds,
            example_tasks: spec
                .example_tasks
                .unwrap_or_default()
                .into_iter()
                .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
                .collect(),
            output_schema_name,
            client: RwLock::new(client),
        })
    }

    /// The model client this agent runs against.
    pub fn client(&self) -> Arc<dyn ModelClient> {
        self.client
            .read()
            .map(|c| Arc::clone(&c))
            .unwrap_or_else(|e| Arc::clone(&e.into_inner()))
    }

    /// Swap the model client (tests and the judge use this to point
    /// one agent at a scripted client).
    pub fn set_client(&self, client: Arc<dyn ModelClient>) {
        if let Ok(mut slot) = self.client.write() {
            *slot = client;
        }
    }

    /// Compose the system and user messages for a task.
    pub fn build_messages(&self, task: &Task) -> Vec<ChatMessage> {
        let mut user_content = format!("Task: {}", task.description);
        if !task.context.is_empty() {
            let context_lines: Vec<String> = task
                .context
                .to_map()
                .iter()
                .map(|(k, v)| {
                    let text = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    format!("- {k}: {text}")
                })
                .collect();
            user_content += &format!("\n\nContext:\n{}", context_lines.join("\n"));
        }
        if let Some(contract) = &task.contract {
            let mut lines = vec![
                "Contract for this task:".to_string(),
                format!("- goal: {}", contract.goal),
            ];
            if !contract.scope.is_empty() {
                lines.push(format!("- in scope: {}", contract.scope.join("; ")));
            }
            if !contract.exclusions.is_empty() {
                lines.push(format!(
                    "- out of scope, do not do these: {}",
                    contract.exclusions.join("; ")
                ));
            }
            if !contract.done_when.is_empty() {
                lines.push(format!("- done when: {}", contract.done_when.join("; ")));
            }
            user_content += &format!("\n\n{}", lines.join("\n"));
        }
        user_content += &format!(
            "\n\nOutput format:\n{}",
            format_instruction(self.output_schema_name)
        );
        vec![
            ChatMessage::new("system", self.system_prompt.clone()),
            ChatMessage::new("user", user_content),
        ]
    }

    /// Append a ledger event. Never raises: the ledger is a side
    /// channel, and a failing side channel must not break the run.
    fn record(
        &self,
        ledger: &Option<Ledger>,
        run_id: &Option<String>,
        kind: &str,
        payload: serde_json::Value,
    ) {
        if let (Some(ledger), Some(run_id)) = (ledger, run_id) {
            let _ = ledger.record(run_id, kind, &self.name, payload);
        }
    }

    fn contract_output(
        &self,
        raw_text: &str,
        base_messages: &[ChatMessage],
        ledger: &Option<Ledger>,
        run_id: &Option<String>,
    ) -> Result<(String, HashMap<String, serde_json::Value>), String> {
        let provider = self.client().provider_for(&self.model);
        if provider == "stub" {
            let text = raw_text.trim().to_string();
            let wrapped = AgentOutput {
                summary: if text.is_empty() {
                    "(no output)".to_string()
                } else {
                    text.clone()
                },
                details: String::new(),
                key_points: Vec::new(),
                extra: HashMap::new(),
            };
            return Ok((text, wrapped.dump()));
        }
        match parse_structured_output(raw_text, self.output_schema_name) {
            Ok(model) => Ok((model.render(), model.dump())),
            Err(first) => {
                self.record(
                    ledger,
                    run_id,
                    "output_repair",
                    serde_json::json!({"error": first.to_string(), "attempt": 1}),
                );
                // One repair attempt with the errors spelled out.
                let mut repair_messages = base_messages.to_vec();
                repair_messages.push(ChatMessage::new("assistant", raw_text.to_string()));
                repair_messages.push(ChatMessage::new(
                    "user",
                    format!(
                        "Your last reply did not match the required output schema:\n{first}\nReply with ONLY a corrected JSON object matching the schema."
                    ),
                ));
                let repaired = self
                    .client()
                    .chat(&self.model, &repair_messages)
                    .map_err(|e| e.to_string())?;
                let (kind, parsed) = parse_model_reply(&repaired);
                let repaired_text = if kind == "final" { parsed } else { repaired };
                let model = parse_structured_output(&repaired_text, self.output_schema_name)
                    .map_err(|e| e.to_string())?;
                Ok((model.render(), model.dump()))
            }
        }
    }

    /// Run one task and return an AgentResult.
    ///
    /// Agents with a toolkit run the think-act-observe loop; agents
    /// without one do a single model call, as before. Uses the
    /// configured model when credentials exist, otherwise the offline
    /// stub. The stub response is always labeled as offline output.
    ///
    /// `ledger` and `run_id` are optional: when given, the run's events
    /// (task assigned, tools called, answer produced, contract repairs,
    /// failures) are appended to the ledger. `store` and `resume_state`
    /// are for durable run state: with a store, tool runs write their
    /// progress after every step; with `resume_state`, a tool run
    /// continues an interrupted run from its completed steps instead
    /// of starting over. Both only affect agents with a toolkit.
    pub fn run(&self, task: Task, mut opts: RunOpts) -> AgentResult {
        let started = Instant::now();
        let task_id = task.id.clone();
        if opts.run_id.is_none() && (opts.ledger.is_some() || opts.store.is_some()) {
            opts.run_id = Some(crate::schemas::new_run_id());
        }
        let run_id = opts.run_id.clone();
        self.record(
            &opts.ledger,
            &run_id,
            "task_assigned",
            serde_json::to_value(&task).unwrap_or(serde_json::Value::Null),
        );
        if !self.toolkit.is_empty() {
            return self.run_with_tools(task, started, opts);
        }
        let messages = self.build_messages(&task);
        let raw = match self.client().chat(&self.model, &messages) {
            Ok(text) => text,
            Err(e) => {
                self.record(
                    &opts.ledger,
                    &run_id,
                    "agent_failed",
                    serde_json::json!({"error": e.to_string()}),
                );
                let mut result =
                    AgentResult::failed(&self.name, &self.department, &task_id, e.to_string());
                result.elapsed_seconds = started.elapsed().as_secs_f64();
                return result;
            }
        };
        let (_kind, parsed) = parse_model_reply(&raw);
        let raw = parsed;
        match self.contract_output(&raw, &messages, &opts.ledger, &run_id) {
            Ok((output, structured)) => {
                let provider = self.client().provider_for(&self.model);
                let usage = self.client().last_usage();
                let mut metadata = HashMap::new();
                metadata.insert(
                    "model".to_string(),
                    serde_json::Value::String(self.model.clone()),
                );
                metadata.insert(
                    "provider".to_string(),
                    serde_json::Value::String(provider.clone()),
                );
                metadata.insert(
                    "stub".to_string(),
                    serde_json::Value::Bool(provider == "stub"),
                );
                metadata.insert(
                    "tokens_in".to_string(),
                    serde_json::Value::from(usage.input),
                );
                metadata.insert(
                    "tokens_out".to_string(),
                    serde_json::Value::from(usage.output),
                );
                metadata.insert(
                    "output_schema".to_string(),
                    serde_json::Value::String(self.output_schema_name.to_string()),
                );
                metadata.insert(
                    "output_validated".to_string(),
                    serde_json::Value::Bool(provider != "stub"),
                );
                let result = AgentResult {
                    agent_name: self.name.clone(),
                    department: self.department.clone(),
                    task_id,
                    output,
                    success: true,
                    confidence: if provider != "stub" { 0.85 } else { 0.5 },
                    error: None,
                    metadata,
                    elapsed_seconds: started.elapsed().as_secs_f64(),
                    tool_calls: Vec::new(),
                    structured,
                };
                self.record(
                    &opts.ledger,
                    &run_id,
                    "output_produced",
                    serde_json::json!({
                        "summary": result.structured.get("summary").cloned().unwrap_or(serde_json::Value::String(String::new())),
                        "structured": result.structured,
                        "output_validated": provider != "stub",
                        "elapsed_seconds": result.elapsed_seconds,
                    }),
                );
                result
            }
            Err(e) => {
                self.record(
                    &opts.ledger,
                    &run_id,
                    "agent_failed",
                    serde_json::json!({"error": e}),
                );
                let mut result = AgentResult::failed(&self.name, &self.department, &task_id, e);
                result.elapsed_seconds = started.elapsed().as_secs_f64();
                result
            }
        }
    }

    /// Run the think-act-observe loop and wrap it in an AgentResult.
    ///
    /// The loop's final answer goes through the same output contract
    /// as the single-shot path: validated against the schema, one
    /// repair attempt on failure. An escalated loop exit (same error
    /// twice, or a budget hit) skips the output contract: there is no
    /// answer to validate. The result is failed, marked escalated,
    /// and says why.
    fn run_with_tools(&self, task: Task, started: Instant, opts: RunOpts) -> AgentResult {
        let task_id = task.id.clone();
        let run_id = opts.run_id.clone();
        let ledger = opts.ledger.clone();
        let store = opts.store.clone();

        // The loop's events go to the ledger and on to the caller's
        // callback, in that order. Tool calls, blocks, plans, and
        // answer checks are the ledger kinds the loop produces.
        let mut user_event = opts.on_event;
        let agent_name = self.name.clone();
        let run_id_for_events = run_id.clone();
        let ledger_for_events = ledger.clone();
        let wrapped_event: EventCallback = Box::new(move |kind, payload| {
            if let Some(ledger) = &ledger_for_events {
                let event_kind = match kind {
                    "tool_finished" => Some("tool_called"),
                    "tool_blocked" => Some("tool_blocked"),
                    "plan_recorded" => Some("plan_recorded"),
                    "answer_checked" => Some("answer_checked"),
                    _ => None,
                };
                if let Some(event_kind) = event_kind {
                    let _ = ledger.record(
                        run_id_for_events.as_deref().unwrap_or(""),
                        event_kind,
                        &agent_name,
                        payload.clone(),
                    );
                }
            }
            if let Some(cb) = user_event.as_mut() {
                cb(kind, payload);
            }
        });

        // Run state is written after every step, and the task map
        // beside it, when a store is attached. A caller hook, when
        // given, runs after the store write.
        let mut user_hook = opts.state_hook;
        let store_for_hook = store.clone();
        let composed_hook: StateHook = Box::new(move |snap| {
            if let Some(store) = &store_for_hook {
                let state = snap.to_json();
                let _ = store.save(&state);
                crate::taskmap::write_map(store, &state);
            }
            if let Some(hook) = user_hook.as_mut() {
                hook(snap);
            }
        });

        // On resume, the map is the run's progress in words. The
        // continuing agent reads it before it starts again.
        let map_text = match opts.map_text {
            Some(text) => Some(text),
            None => match (&opts.resume_state, &store, &run_id) {
                (Some(_), Some(store), Some(run_id)) => {
                    crate::taskmap::read_map(store, run_id).ok()
                }
                _ => None,
            },
        };

        let mut loop_opts = LoopOpts {
            token_budget: opts.token_budget.or(self.token_budget),
            time_budget_seconds: opts.time_budget_seconds.or(self.time_budget_seconds),
            run_id: run_id.clone(),
            resume_state: opts.resume_state,
            map_text,
            run_context: opts.run_context,
            registry: opts.registry,
            on_event: Some(wrapped_event),
            state_hook: Some(composed_hook),
        };
        let (raw_output, tool_calls, usage) = match run_tool_loop(self, &task, &mut loop_opts) {
            Ok(out) => out,
            Err(e) => {
                self.record(
                    &ledger,
                    &run_id,
                    "agent_failed",
                    serde_json::json!({"error": e}),
                );
                let mut result = AgentResult::failed(&self.name, &self.department, &task_id, e);
                result.elapsed_seconds = started.elapsed().as_secs_f64();
                return result;
            }
        };
        let provider = self.client().provider_for(&self.model);
        let mut metadata = HashMap::new();
        metadata.insert(
            "model".to_string(),
            serde_json::Value::String(self.model.clone()),
        );
        metadata.insert(
            "provider".to_string(),
            serde_json::Value::String(provider.clone()),
        );
        metadata.insert(
            "stub".to_string(),
            serde_json::Value::Bool(provider == "stub"),
        );
        metadata.insert(
            "tokens_in".to_string(),
            serde_json::Value::from(usage.input),
        );
        metadata.insert(
            "tokens_out".to_string(),
            serde_json::Value::from(usage.output),
        );
        metadata.insert(
            "toolkit".to_string(),
            serde_json::Value::Array(
                self.toolkit
                    .iter()
                    .map(|t| serde_json::Value::String(t.clone()))
                    .collect(),
            ),
        );
        metadata.insert(
            "tool_calls".to_string(),
            serde_json::Value::from(tool_calls.len()),
        );
        metadata.insert(
            "output_schema".to_string(),
            serde_json::Value::String(self.output_schema_name.to_string()),
        );
        if usage.escalated {
            let reason = usage
                .escalation_reason
                .clone()
                .unwrap_or_else(|| "run escalated".to_string());
            metadata.insert("escalated".to_string(), serde_json::Value::Bool(true));
            metadata.insert(
                "escalation_reason".to_string(),
                serde_json::Value::String(reason.clone()),
            );
            metadata.insert(
                "output_validated".to_string(),
                serde_json::Value::Bool(false),
            );
            self.record(
                &ledger,
                &run_id,
                "run_escalated",
                serde_json::json!({"reason": reason, "tool_calls": tool_calls.len()}),
            );
            return AgentResult {
                agent_name: self.name.clone(),
                department: self.department.clone(),
                task_id,
                output: raw_output,
                success: false,
                confidence: 0.0,
                error: Some(reason),
                metadata,
                elapsed_seconds: started.elapsed().as_secs_f64(),
                tool_calls,
                structured: HashMap::new(),
            };
        }
        match self.contract_output(&raw_output, &self.build_messages(&task), &ledger, &run_id) {
            Ok((output, structured)) => {
                metadata.insert(
                    "output_validated".to_string(),
                    serde_json::Value::Bool(provider != "stub"),
                );
                let result = AgentResult {
                    agent_name: self.name.clone(),
                    department: self.department.clone(),
                    task_id,
                    output,
                    success: true,
                    confidence: if provider != "stub" { 0.85 } else { 0.5 },
                    error: None,
                    metadata,
                    elapsed_seconds: started.elapsed().as_secs_f64(),
                    tool_calls,
                    structured,
                };
                self.record(
                    &ledger,
                    &run_id,
                    "output_produced",
                    serde_json::json!({
                        "summary": result.structured.get("summary").cloned().unwrap_or(serde_json::Value::String(String::new())),
                        "structured": result.structured,
                        "output_validated": provider != "stub",
                        "tool_calls": result.tool_calls.len(),
                        "elapsed_seconds": result.elapsed_seconds,
                    }),
                );
                result
            }
            Err(e) => {
                self.record(
                    &ledger,
                    &run_id,
                    "agent_failed",
                    serde_json::json!({"error": e}),
                );
                let mut result = AgentResult::failed(&self.name, &self.department, &task_id, e);
                result.elapsed_seconds = started.elapsed().as_secs_f64();
                result.tool_calls = tool_calls;
                result
            }
        }
    }

    /// Short public description of this agent.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "title": self.title,
            "department": self.department,
            "specialty": self.specialty,
            "model": self.model,
            "tools": self.toolkit,
            "toolkit": self.toolkit,
            "output_schema": self.output_schema_name,
            "example_tasks": self.example_tasks,
        })
    }
}

/// Per-run options for `Agent::run`. Extended by later PRs as the
/// ledger and run state land.
#[derive(Default)]
pub struct RunOpts {
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<f64>,
    pub run_id: Option<String>,
    /// A saved state dict to resume from, if any.
    pub resume_state: Option<serde_json::Value>,
    /// The run's task map in words, read back on resume.
    pub map_text: Option<String>,
    pub run_context: Option<std::rc::Rc<std::cell::RefCell<crate::tool_loop::RunContext>>>,
    pub registry: Option<std::sync::Arc<crate::registry::AgentRegistry>>,
    /// The run ledger, when events should be recorded.
    pub ledger: Option<Ledger>,
    /// The run state store, when progress should be written to disk.
    pub store: Option<RunStore>,
    /// Called as `on_event(kind, payload)` with tool_started,
    /// tool_finished, tool_blocked, plan_recorded, and answer_checked.
    pub on_event: Option<crate::tool_loop::EventCallback>,
    /// Called with a snapshot after every step, when run state is
    /// being written.
    pub state_hook: Option<crate::tool_loop::StateHook>,
}

/// Parse one model reply into (kind, payload).
///
/// Kinds: "action" -> the raw JSON after ACTION:, "final" -> answer
/// text, "malformed" -> error description. Plain prose counts as the
/// answer, which keeps the loop safe by default for small models and
/// the offline stub.
pub fn parse_model_reply(text: &str) -> (&'static str, String) {
    let stripped = text.trim();
    if let Some(rest) = stripped.strip_prefix("FINAL:") {
        return ("final", rest.trim().to_string());
    }
    if let Some(rest) = stripped.strip_prefix("ACTION:") {
        let raw = rest.trim();
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(payload) => {
                if !payload.is_object() || payload.get("tool").is_none() {
                    return (
                        "malformed",
                        "ACTION must be {\"tool\": \"<name>\", \"args\": {...}}".to_string(),
                    );
                }
                if !payload.get("args").map(|a| a.is_object()).unwrap_or(true) {
                    return ("malformed", "\"args\" must be an object".to_string());
                }
                if !payload.get("tool").map(|t| t.is_string()).unwrap_or(false) {
                    return ("malformed", "\"tool\" must be a string".to_string());
                }
                return ("action", raw.to_string());
            }
            Err(e) => return ("malformed", format!("invalid JSON: {e}")),
        }
    }
    ("final", text.to_string())
}

/// The tool-use instructions appended to the system prompt.
pub fn tool_prompt(tool_names: &[String]) -> String {
    let mut lines = vec![
        "You have these tools. Use them when they help.".to_string(),
        String::new(),
    ];
    for name in tool_names {
        if let Some(def) = get_tool_def(name) {
            let params = def
                .parameters
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .map(|(key, spec)| {
                            let ty = spec.get("type").and_then(|t| t.as_str()).unwrap_or("?");
                            format!("{key} ({ty})")
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            lines.push(format!("- {}({}): {}", def.name, params, def.description));
        }
    }
    lines.extend([
        String::new(),
        "Reply with exactly one of these:".to_string(),
        "ACTION: {\"tool\": \"<name>\", \"args\": {...}}".to_string(),
        "FINAL: <your answer>".to_string(),
        String::new(),
        "After each ACTION you receive an OBSERVATION: line with the result. Use observations to decide the next step. When you have what you need, reply with FINAL: and your answer.".to_string(),
        String::new(),
        "For a task with more than one step, you may begin with a short plan: make your first reply a line starting with PLAN: followed by the steps you intend to take, and nothing else. You will then be told to begin. Skip the plan for one-step tasks.".to_string(),
        String::new(),
        "Before your FINAL: answer is accepted, you will be asked to check it once against the task: look for errors, unmet requirements, and claims your observations do not support.".to_string(),
    ]);
    lines.join("\n")
}
