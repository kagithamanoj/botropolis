//! Durable run state for tool-using agent runs.
//!
//! The ledger records what happened; run state is what a run needs to
//! continue. While a tool loop runs, it writes its progress to a JSON
//! file after every step: the task, the contract, and each completed
//! tool call with its result. If the process dies mid-run, the state on
//! disk is where the run stopped, and `botropolis resume <run_id>`
//! rebuilds the conversation from the completed steps and carries on
//! from the first unfinished one instead of starting over.
//!
//! State files live in `botropolis/data/runs/` (gitignored), next to
//! the usage log. `BOTROPOLIS_RUNS_DIR` moves the directory, which is
//! how tests keep runs hermetic. The JSON shape matches the Python
//! side (`botropolis/core/runstate.py`), so either side can resume the
//! other's runs.

use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::schemas::{Context, Contract, Task};

/// Return the run state directory, creating it on first use.
pub fn runs_dir() -> PathBuf {
    if let Ok(override_path) = env::var("BOTROPOLIS_RUNS_DIR") {
        if !override_path.is_empty() {
            let root = PathBuf::from(override_path);
            let _ = std::fs::create_dir_all(&root);
            return root;
        }
    }
    let root = crate::paths::data_dir().join("runs");
    let _ = std::fs::create_dir_all(&root);
    root
}

/// Reads and writes per-run state files.
#[derive(Debug, Clone)]
pub struct RunStore {
    pub root: PathBuf,
}

impl RunStore {
    pub fn new(root: Option<PathBuf>) -> RunStore {
        RunStore {
            root: root.unwrap_or_else(runs_dir),
        }
    }

    pub fn path_for(&self, run_id: &str) -> PathBuf {
        self.root.join(format!("{run_id}.json"))
    }

    /// Write one run's state, replacing any earlier snapshot.
    pub fn save(&self, state: &Value) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let mut state = state.clone();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        state["updated_at"] = Value::from(now);
        let run_id = state
            .get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let path = self.path_for(&run_id);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read one run's state. Returns an error when absent.
    pub fn load(&self, run_id: &str) -> Result<Value, String> {
        let path = self.path_for(run_id);
        if !path.is_file() {
            return Err(format!("No saved run with id {run_id}"));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    pub fn exists(&self, run_id: &str) -> bool {
        self.path_for(run_id).is_file()
    }
}

impl Default for RunStore {
    fn default() -> Self {
        RunStore::new(None)
    }
}

/// Rebuild the Task a saved run was working on.
pub fn task_from_state(state: &Value) -> Task {
    let data = state.get("task").cloned().unwrap_or(Value::Null);
    let context_data = data.get("context").cloned().unwrap_or(Value::Null);
    let mut context = Context::new();
    if let Value::Object(map) = &context_data {
        for (key, value) in map {
            match key.as_str() {
                "chat_history" => {
                    context.chat_history = value.as_str().unwrap_or("").to_string();
                }
                "teammates_so_far" => {
                    context.teammates_so_far = value.as_str().unwrap_or("").to_string();
                }
                _ => {
                    context.extra.insert(key.clone(), value.clone());
                }
            }
        }
    }
    let contract = Contract::from_map(state.get("contract").or_else(|| data.get("contract")));
    let mut task = Task::new(
        data.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    task.department = data
        .get("department")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    task.agent_name = data
        .get("agent_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    task.context = context;
    task.contract = contract;
    task.needs_approval = data
        .get("needs_approval")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    task.approved = data
        .get("approved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    task.id = data
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "resumed".to_string());
    task
}

/// The path of one run's state file, for callers that need it.
pub fn state_path(root: &Path, run_id: &str) -> PathBuf {
    root.join(format!("{run_id}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::new(Some(dir.path().to_path_buf()));
        let state = json!({
            "run_id": "run1",
            "agent": "coder",
            "task": {"description": "do math"},
            "steps": [],
            "status": "running",
        });
        store.save(&state).unwrap();
        assert!(store.exists("run1"));
        let loaded = store.load("run1").unwrap();
        assert_eq!(loaded["agent"], json!("coder"));
        assert!(loaded["updated_at"].as_f64().unwrap() > 0.0);
        assert!(store.load("nope").is_err());
    }

    #[test]
    fn task_from_state_rebuilds_the_task() {
        let state = json!({
            "run_id": "run1",
            "agent": "coder",
            "task": {
                "description": "do math",
                "department": "code",
                "agent_name": "coder",
                "context": {"chat_history": "user: hi"},
                "needs_approval": ["shell"],
                "approved": true,
                "id": "abc123",
            },
            "contract": {"goal": "finish the math", "scope": [], "exclusions": [], "done_when": [], "checks": []},
        });
        let task = task_from_state(&state);
        assert_eq!(task.description, "do math");
        assert_eq!(task.department.as_deref(), Some("code"));
        assert_eq!(task.context.chat_history, "user: hi");
        assert_eq!(task.needs_approval, vec!["shell"]);
        assert!(task.approved);
        assert_eq!(task.id, "abc123");
        assert_eq!(task.contract.as_ref().unwrap().goal, "finish the math");
    }
}
