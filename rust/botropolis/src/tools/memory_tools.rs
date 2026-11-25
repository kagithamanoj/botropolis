//! Long-term memory tools: memory_save and memory_recall.
//!
//! The notebook is a page of notes the whole company shares. These
//! tools are the searchable layer under it: individual facts, saved by
//! the agent that learned them, that any later run can pull back by
//! keyword. The store is `crate::memory`; the tools resolve the shared
//! store at call time, so the server, the CLI, and tests can pin their
//! own instance with `set_memory_store()`.
//!
//! memory_save is agent-aware: the tool loop passes the calling
//! agent's name in at dispatch, so a memory always records who learned
//! it. The model never supplies the name itself.

use serde_json::{json, Value};

use super::builtin::{arg_i64, arg_str};
use crate::memory::get_memory_store;

fn tag_list(tags: Option<&Value>) -> Vec<String> {
    match tags {
        None => Vec::new(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|t| !t.is_empty())
            .collect(),
        Some(_) => Vec::new(),
    }
}

/// Save one durable fact to the company memory.
pub fn memory_save(args: &Value) -> Result<Value, String> {
    let fact = arg_str(args, "fact")?.trim().to_string();
    if fact.is_empty() {
        return Ok(json!({"error": "Nothing to save: fact was empty."}));
    }
    let importance = arg_i64(args, "importance", 3);
    let agent = args
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tags = tag_list(args.get("tags"));
    let memory = get_memory_store().save(&fact, &agent, importance, &tags)?;
    Ok(json!({"saved": true, "memory": memory}))
}

/// Search the company memory for facts from earlier work.
pub fn memory_recall(args: &Value) -> Result<Value, String> {
    let query = arg_str(args, "query")?.trim().to_string();
    if query.is_empty() {
        return Ok(json!({"error": "Nothing to search for: query was empty."}));
    }
    let limit = arg_i64(args, "limit", 5).clamp(1, 20) as usize;
    let found = get_memory_store().recall(&query, limit)?;
    Ok(json!({
        "query": query,
        "count": found.len(),
        "memories": found,
    }))
}
