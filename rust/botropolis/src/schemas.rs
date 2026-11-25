//! Shared data schemas for Botropolis.
//!
//! These are the contracts passed between the CEO orchestrator, the
//! department agents, and the API server. Field names and JSON shapes
//! match the Python dataclasses in `botropolis/core/schemas.py`, so
//! run state written by either side reads the same on the other.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Everything an agent may know beyond the task description.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Context {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chat_history: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memories: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub teammates_so_far: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty", flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    /// Non-empty fields as a plain map, for prompts and logs.
    pub fn to_map(&self) -> HashMap<String, serde_json::Value> {
        let mut data = HashMap::new();
        if !self.chat_history.is_empty() {
            data.insert(
                "chat_history".to_string(),
                serde_json::Value::String(self.chat_history.clone()),
            );
        }
        if !self.memories.is_empty() {
            data.insert(
                "memories".to_string(),
                serde_json::Value::String(self.memories.clone()),
            );
        }
        if !self.teammates_so_far.is_empty() {
            data.insert(
                "teammates_so_far".to_string(),
                serde_json::Value::String(self.teammates_so_far.clone()),
            );
        }
        for (key, value) in &self.extra {
            data.insert(key.clone(), value.clone());
        }
        data
    }

    pub fn is_empty(&self) -> bool {
        self.to_map().is_empty()
    }
}

/// One machine check that verifies a contracted run is done.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractCheck {
    pub command: String,
    #[serde(default)]
    pub expected_exit_code: i32,
    #[serde(default)]
    pub name: String,
}

impl ContractCheck {
    pub fn new(command: impl Into<String>) -> Self {
        ContractCheck {
            command: command.into(),
            expected_exit_code: 0,
            name: String::new(),
        }
    }
}

/// The outcome of one ContractCheck, attached to a report as evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckResult {
    pub command: String,
    pub expected_exit_code: i32,
    pub exit_code: Option<i32>,
    pub passed: bool,
    #[serde(default)]
    pub output_tail: String,
}

/// The agreement for one run: what is being done, what is in and out
/// of scope, and what counts as done.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Contract {
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub checks: Vec<ContractCheck>,
}

impl Contract {
    pub fn from_map(data: Option<&serde_json::Value>) -> Option<Contract> {
        let data = data?;
        if data.is_null() {
            return None;
        }
        let parsed: Contract = serde_json::from_value(data.clone()).ok()?;
        Some(parsed)
    }
}

/// A unit of work assigned to one agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub context: Context,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
    #[serde(default)]
    pub needs_approval: Vec<String>,
    #[serde(default)]
    pub approved: bool,
    #[serde(default = "default_task_id")]
    pub id: String,
}

fn default_task_id() -> String {
    new_short_id()
}

/// A short random hex id, like the Python side's `uuid4().hex[:8]`.
pub fn new_short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos ^ (pid << 64) ^ ((nanos >> 17) * 0x9e37_79b9_7f4a_7c15);
    format!("{:08x}", (mixed & 0xffff_ffff) as u64)
}

/// A 12-hex-char run id, like the Python side's `uuid4().hex[:12]`.
pub fn new_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos ^ (pid << 32) ^ 0x9e37_79b9_7f4a_7c15;
    format!("{:012x}", (mixed & 0xffff_ffff_ffff) as u64)
}

impl Task {
    pub fn new(description: impl Into<String>) -> Self {
        Task {
            description: description.into(),
            department: None,
            agent_name: None,
            context: Context::new(),
            contract: None,
            needs_approval: Vec::new(),
            approved: false,
            id: new_short_id(),
        }
    }
}

/// One tool invocation made by an agent during its run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default = "default_true")]
    pub success: bool,
    #[serde(default)]
    pub result_preview: String,
    #[serde(default)]
    pub elapsed_ms: f64,
}

fn default_true() -> bool {
    true
}

impl ToolCall {
    pub fn new(tool: impl Into<String>, args: serde_json::Value) -> Self {
        ToolCall {
            tool: tool.into(),
            args,
            success: true,
            result_preview: String::new(),
            elapsed_ms: 0.0,
        }
    }
}

/// The outcome of one agent running one task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub agent_name: String,
    pub department: String,
    pub task_id: String,
    pub output: String,
    #[serde(default = "default_true")]
    pub success: bool,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub structured: HashMap<String, serde_json::Value>,
}

fn default_confidence() -> f64 {
    0.5
}

impl AgentResult {
    pub fn failed(
        agent_name: &str,
        department: &str,
        task_id: &str,
        error: impl Into<String>,
    ) -> Self {
        AgentResult {
            agent_name: agent_name.to_string(),
            department: department.to_string(),
            task_id: task_id.to_string(),
            output: String::new(),
            success: false,
            confidence: 0.0,
            error: Some(error.into()),
            metadata: HashMap::new(),
            elapsed_seconds: 0.0,
            tool_calls: Vec::new(),
            structured: HashMap::new(),
        }
    }
}

/// The CEO's final report for one user request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyReport {
    pub request: String,
    #[serde(default)]
    pub departments_involved: Vec<String>,
    #[serde(default)]
    pub results: Vec<AgentResult>,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
    #[serde(default)]
    pub evidence: Vec<CheckResult>,
    #[serde(default)]
    pub judge: HashMap<String, serde_json::Value>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub created_at: f64,
}

fn default_status() -> String {
    "done".to_string()
}

impl CompanyReport {
    /// True when every agent result succeeded.
    pub fn successful(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(|r| r.success)
    }
}

/// One agent's turn inside a team session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRound {
    pub round_number: u32,
    pub agent_name: String,
    pub department: String,
    pub result: AgentResult,
}

/// The CEO's report for one team collaboration session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamReport {
    pub request: String,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub rounds: Vec<TeamRound>,
    #[serde(default)]
    pub synthesis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
    #[serde(default)]
    pub evidence: Vec<CheckResult>,
    #[serde(default)]
    pub judge: HashMap<String, serde_json::Value>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub created_at: f64,
}

impl TeamReport {
    /// True when every team round succeeded.
    pub fn successful(&self) -> bool {
        !self.rounds.is_empty() && self.rounds.iter().all(|r| r.result.success)
    }
}
