//! Usage analytics: who ran, how long it took, what the model reported.
//!
//! Every agent invocation (through the CEO or direct) is appended as
//! one JSON line to a local log. Nothing leaves the machine; it is just
//! a record of who ran, how long it took, and what the model reported.
//!
//! The log lives at `botropolis/data/usage.jsonl` (gitignored).
//! `BOTROPOLIS_USAGE_LOG` moves it, which is how tests keep the log
//! hermetic. Mirrors `botropolis/core/usage.py`.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where the usage log lives.
pub fn usage_log_path() -> PathBuf {
    if let Ok(override_path) = env::var("BOTROPOLIS_USAGE_LOG") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    crate::paths::data_dir().join("usage.jsonl")
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Append one agent invocation to the usage log.
///
/// Never raises: analytics must not break the request path. Returns
/// the row that was written (or would have been written).
#[allow(clippy::too_many_arguments)]
pub fn log_invocation(
    agent_name: &str,
    department: &str,
    model: &str,
    provider: &str,
    latency_ms: f64,
    tokens_in: u64,
    tokens_out: u64,
    success: bool,
    tool_calls: usize,
    tool_names: &[String],
) -> Value {
    let row = json!({
        "ts": now_secs(),
        "agent": agent_name,
        "department": department,
        "model": model,
        "provider": provider,
        "latency_ms": (latency_ms * 100.0).round() / 100.0,
        "tokens_in": tokens_in,
        "tokens_out": tokens_out,
        "success": success,
        "tool_calls": tool_calls,
        "tools": tool_names,
    });
    let path = usage_log_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{}", serde_json::to_string(&row).unwrap_or_default());
    }
    row
}

#[derive(Default)]
struct AgentTotals {
    calls: u64,
    successes: u64,
    latency_ms_total: f64,
    tokens_in: u64,
    tokens_out: u64,
    stub_calls: u64,
    tool_calls: u64,
    tools: HashMap<String, u64>,
    models: Vec<String>,
}

/// Aggregate the usage log into per-agent totals.
///
/// Returns `{"agents": {name: {...}}, "totals": {...}}`. Reads never
/// raise; a missing log just yields empty totals.
pub fn get_summary() -> Value {
    let mut per_agent: HashMap<String, AgentTotals> = HashMap::new();
    let mut calls = 0u64;
    let mut successes = 0u64;
    let mut stub_calls = 0u64;
    let mut tool_calls_total = 0u64;
    if let Ok(text) = std::fs::read_to_string(usage_log_path()) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let row: Value = match serde_json::from_str(line) {
                Ok(row) => row,
                Err(_) => continue,
            };
            let name = row
                .get("agent")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let entry = per_agent.entry(name).or_default();
            entry.calls += 1;
            entry.latency_ms_total += row
                .get("latency_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            entry.tokens_in += row.get("tokens_in").and_then(|v| v.as_u64()).unwrap_or(0);
            entry.tokens_out += row.get("tokens_out").and_then(|v| v.as_u64()).unwrap_or(0);
            if row
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                entry.successes += 1;
                successes += 1;
            }
            if row.get("provider").and_then(|v| v.as_str()) == Some("stub") {
                entry.stub_calls += 1;
                stub_calls += 1;
            }
            let tools = row.get("tool_calls").and_then(|v| v.as_u64()).unwrap_or(0);
            entry.tool_calls += tools;
            tool_calls_total += tools;
            if let Some(names) = row.get("tools").and_then(|v| v.as_array()) {
                for tool in names {
                    if let Some(name) = tool.as_str() {
                        *entry.tools.entry(name.to_string()).or_insert(0) += 1;
                    }
                }
            }
            if let Some(model) = row.get("model").and_then(|v| v.as_str()) {
                if !model.is_empty() && !entry.models.contains(&model.to_string()) {
                    entry.models.push(model.to_string());
                }
            }
            calls += 1;
        }
    }
    let mut names: Vec<&String> = per_agent.keys().collect();
    names.sort();
    let mut agents = serde_json::Map::new();
    for name in names {
        let entry = &per_agent[name];
        let n = entry.calls;
        let mut tools: Vec<(&String, &u64)> = entry.tools.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let mut tool_map = serde_json::Map::new();
        for (tool, count) in tools.into_iter().take(5) {
            tool_map.insert(tool.clone(), json!(count));
        }
        let mut models = entry.models.clone();
        models.sort();
        agents.insert(
            name.clone(),
            json!({
                "calls": n,
                "successes": entry.successes,
                "avg_latency_ms": if n > 0 { (entry.latency_ms_total / n as f64 * 100.0).round() / 100.0 } else { 0.0 },
                "tokens_in": entry.tokens_in,
                "tokens_out": entry.tokens_out,
                "tokens_total": entry.tokens_in + entry.tokens_out,
                "stub_calls": entry.stub_calls,
                "tool_calls": entry.tool_calls,
                "tools": tool_map,
                "models": models,
            }),
        );
    }
    json!({
        "agents": agents,
        "totals": {
            "calls": calls,
            "successes": successes,
            "stub_calls": stub_calls,
            "tool_calls": tool_calls_total,
            "log_path": usage_log_path().to_string_lossy(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_env;

    #[test]
    fn log_and_summarize() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usage.jsonl");
        std::env::set_var("BOTROPOLIS_USAGE_LOG", &log);
        log_invocation(
            "scout",
            "research",
            "stub",
            "stub",
            12.5,
            0,
            0,
            true,
            0,
            &[],
        );
        log_invocation(
            "scout",
            "research",
            "stub",
            "stub",
            7.5,
            0,
            0,
            true,
            1,
            &["calculator".to_string()],
        );
        log_invocation(
            "coder",
            "code",
            "gpt-4o",
            "openai",
            100.0,
            50,
            20,
            true,
            0,
            &[],
        );
        let summary = get_summary();
        assert_eq!(summary["totals"]["calls"], json!(3));
        assert_eq!(summary["totals"]["stub_calls"], json!(2));
        assert_eq!(summary["agents"]["scout"]["calls"], json!(2));
        assert_eq!(summary["agents"]["scout"]["avg_latency_ms"], json!(10.0));
        assert_eq!(summary["agents"]["scout"]["tool_calls"], json!(1));
        assert_eq!(summary["agents"]["coder"]["tokens_total"], json!(70));
        assert_eq!(summary["agents"]["scout"]["tools"]["calculator"], json!(1));
        std::env::remove_var("BOTROPOLIS_USAGE_LOG");
    }

    #[test]
    fn missing_log_is_empty_totals() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BOTROPOLIS_USAGE_LOG", dir.path().join("no-such-log.jsonl"));
        let summary = get_summary();
        assert_eq!(summary["totals"]["calls"], json!(0));
        assert!(summary["agents"].as_object().unwrap().is_empty());
        std::env::remove_var("BOTROPOLIS_USAGE_LOG");
    }
}
