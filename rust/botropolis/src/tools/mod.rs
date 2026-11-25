//! Builtin tools shipped with Botropolis.
//!
//! Each tool has a name, a description, a JSON-schema-style parameter
//! map, and an execute function. Agents declare which tools they may
//! actually call in the `toolkit` field of their spec YAML.
//!
//! This module holds the tool catalog: the names, descriptions, and
//! parameter shapes every agent spec may reference. The implementations
//! live in the submodules and land with the tool port; dispatching a
//! tool whose implementation has not landed yet is a plain error, not
//! a silent success.

pub mod builtin;
pub mod expr;

use serde_json::{json, Value};

/// One tool's public description: name, what it does, and its parameters.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
    pub agent_aware: bool,
    pub caller_aware: bool,
    pub run_aware: bool,
}

fn param(param_type: &str, description: &str, required: bool) -> Value {
    let mut obj = json!({"type": param_type, "description": description});
    if required {
        obj["required"] = Value::Bool(true);
    }
    obj
}

/// The full catalog of tool names agents may put in a toolkit.
/// Names and argument shapes match the Python tools in
/// `botropolis/tools/`.
pub fn tool_catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "web_search",
            description: "Search the web for current information. Returns ranked results with titles, urls, and snippets.",
            parameters: json!({
                "query": param("string", "The search query", true),
                "num_results": param("integer", "Max results", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "web_fetch",
            description: "Fetch a URL and return its readable text with HTML stripped. Use after web_search to read a promising result in full.",
            parameters: json!({
                "url": param("string", "The URL to fetch", true),
                "max_chars": param("integer", "Truncate text after N chars", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "shell",
            description: "Run a shell command inside the agent workspace directory and capture stdout/stderr. Destructive commands are refused.",
            parameters: json!({
                "command": param("string", "Shell command to run", true),
                "timeout": param("integer", "Kill the command after N seconds", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "calculator",
            description: "Evaluate an arithmetic expression safely.",
            parameters: json!({
                "expression": param("string", "e.g. '2 + 3 * 4'", true),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "read_file",
            description: "Read a UTF-8 text file inside the agent workspace.",
            parameters: json!({
                "path": param("string", "Path inside the workspace, e.g. 'notes/todo.txt'", true),
                "max_chars": param("integer", "Truncate after N chars", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "write_file",
            description: "Write text to a file inside the agent workspace, creating parents.",
            parameters: json!({
                "path": param("string", "Path inside the workspace, e.g. 'notes/todo.txt'", true),
                "content": param("string", "Text to write", true),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "list_dir",
            description: "List entries of a directory inside the agent workspace.",
            parameters: json!({
                "path": param("string", "Directory inside the workspace", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "current_time",
            description: "Return the current UTC time as an ISO 8601 string.",
            parameters: json!({}),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "notes_append",
            description: "Append a timestamped entry to the shared company notebook. Use it to remember durable facts across runs: user preferences, decisions made, things to follow up on. Keep entries short.",
            parameters: json!({
                "text": param("string", "The note to append", true),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "notes_read",
            description: "Read the shared company notebook. Returns the most recent entries (capped). Check it at the start of a task when past context might matter.",
            parameters: json!({}),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "gmail_search",
            description: "Search Gmail with a Gmail search query (from:, subject:, is:unread, newer_than:7d, ...). Returns sender, subject, date, and message id for each match. Read-only.",
            parameters: json!({
                "query": param("string", "Gmail search query", true),
                "max_results": param("integer", "Max results", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "gmail_read",
            description: "Read a single Gmail message by its id (from gmail_search).",
            parameters: json!({
                "message_id": param("string", "The message id", true),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "gmail_draft",
            description: "Create a Gmail draft. Drafts only, nothing is sent.",
            parameters: json!({
                "to": param("string", "Recipient address", true),
                "subject": param("string", "Draft subject", true),
                "body": param("string", "Draft body", true),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "calendar_agenda",
            description: "List upcoming calendar events for the next few days.",
            parameters: json!({
                "days": param("integer", "Days ahead to list", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "calendar_create_event",
            description: "Create a private calendar event (no attendees).",
            parameters: json!({
                "title": param("string", "Event title", true),
                "start": param("string", "Start time, ISO 8601", true),
                "end": param("string", "End time, ISO 8601", true),
                "description": param("string", "Event description", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "math",
            description: "Math with an expression evaluator: arithmetic, functions, variables, and polynomial algebra (solve, simplify, factor, expand, diff, integrate, limit, series, eval). This is an evaluator, not symbolic algebra: it cannot do general symbolic work the way sympy does.",
            parameters: json!({
                "operation": param("string", "One of: solve, simplify, factor, expand, diff, integrate, limit, series, eval", true),
                "expression": param("string", "e.g. 'x^2 - 5*x + 6 = 0' for solve, 'sin(x)' to integrate", true),
                "variable": param("string", "The variable to work in", false),
                "point": param("string", "Point for limit and series, e.g. '0' or 'oo'", false),
                "lower": param("string", "Lower bound for a definite integral", false),
                "upper": param("string", "Upper bound for a definite integral", false),
                "terms": param("integer", "Number of terms for series", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "job_search",
            description: "Search remote job postings (RemoteOK) by keywords matched against each posting's title, company, and tags. Every word in keywords must match. Returns compact postings with title, company, tags, location, url, and date.",
            parameters: json!({
                "keywords": param("string", "Keywords to match, e.g. 'python django'", true),
                "limit": param("integer", "Max postings to return (default 10, cap 25)", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "job_apply",
            description: "Prepare a job application: check the resume exists in the workspace and record the application (title, company, apply url, resume) in the local applications log. This does NOT submit to the employer; it returns the apply url so a human can finish the submission. Recording the same title and company twice returns a warning instead of a duplicate.",
            parameters: json!({
                "title": param("string", "Job title, as it appears in the posting", true),
                "company": param("string", "Company name, as it appears in the posting", true),
                "url": param("string", "The posting's apply url", true),
                "resume": param("string", "Resume path inside the workspace, e.g. 'resume.txt'", true),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "memory_save",
            description: "Save one durable fact to the company memory so future tasks can find it: a decision, a person's name, a number, a preference, or a fact you learned during this task. Save facts as you learn them, one fact per call, in plain words a teammate could search for later. importance runs 1 to 5: 5 for things that must not be forgotten (names, commitments, hard numbers), 1 for minor details.",
            parameters: json!({
                "fact": param("string", "The fact to remember, one or two sentences", true),
                "importance": param("integer", "1 (minor) to 5 (must not forget)", false),
                "tags": param("string", "Comma-separated search tags, e.g. 'billing, acme'", false),
            }),
            agent_aware: true, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "memory_recall",
            description: "Search the company memory for facts saved during earlier work. Call this before answering anything about past work, earlier decisions, or things the company already learned, instead of guessing. Returns the best matches with who saved them and how important they were marked.",
            parameters: json!({
                "query": param("string", "Words describing the fact you need", true),
                "limit": param("integer", "Max memories to return", false),
            }),
            agent_aware: false, caller_aware: false, run_aware: false,
        },
        ToolDef {
            name: "consult",
            description: "Ask a specialist agent one focused question and get their answer back. Use this when a task turns on another agent's field, security, data, research, cloud, instead of guessing outside your own. Name the agent exactly as it appears on the roster. You cannot consult yourself, a consulted agent cannot consult further, and you get at most three consults in one run, so make each question count.",
            parameters: json!({
                "agent": param("string", "The roster name of the agent to ask, e.g. 'auditor'", true),
                "question": param("string", "One focused question, with the facts the specialist needs", true),
            }),
            agent_aware: false, caller_aware: true, run_aware: true,
        },
    ]
}

/// Every tool name agents may reference.
pub fn tool_names() -> Vec<&'static str> {
    tool_catalog().iter().map(|t| t.name).collect()
}

/// Look up one tool's catalog entry by name.
pub fn get_tool_def(name: &str) -> Option<ToolDef> {
    tool_catalog().into_iter().find(|t| t.name == name)
}

/// True when the name is a known tool.
pub fn known_tool(name: &str) -> bool {
    get_tool_def(name).is_some()
}

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::registry::AgentRegistry;
use crate::tool_loop::RunContext;

/// What a tool call needs beyond its own arguments: the calling
/// agent's name, the run's shared context (consult depth and budget),
/// and the registry consult resolves specialists from.
pub struct ToolCtx {
    pub agent_name: String,
    pub run_context: Rc<RefCell<RunContext>>,
    pub registry: Option<Arc<AgentRegistry>>,
}

impl ToolCtx {
    pub fn new(agent_name: impl Into<String>) -> Self {
        ToolCtx {
            agent_name: agent_name.into(),
            run_context: Rc::new(RefCell::new(RunContext::default())),
            registry: None,
        }
    }
}

/// Run one tool by name. Returns the tool's result as JSON, or an
/// error string the loop records as a failed call. Tools whose
/// implementation has not landed yet fail plainly instead of
/// pretending to work.
pub fn dispatch(name: &str, args: &Value, ctx: &ToolCtx) -> Result<Value, String> {
    let _ = ctx;
    match name {
        "calculator" => builtin::calculator(args),
        _ => {
            if known_tool(name) {
                Err(format!(
                    "tool {name} is not implemented yet in the Rust port"
                ))
            } else {
                Err(format!("Unknown tool: {name}"))
            }
        }
    }
}
