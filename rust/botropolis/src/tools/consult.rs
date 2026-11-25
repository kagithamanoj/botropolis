//! The consult tool: ask a specialist agent a question mid-task.
//!
//! Any agent with a toolkit can consult another agent: the named agent
//! runs the question as its own task and its answer comes back as the
//! tool result. This is how an agent gets a specialist's judgment
//! instead of guessing outside its field.
//!
//! Guards, all enforced here rather than trusted to the model:
//!
//! - An agent cannot consult itself.
//! - Depth is one: a consulted run runs with depth 1 in its run
//!   context, and a consult attempted at depth 1 is refused, so
//!   consults never chain.
//! - A run gets at most MAX_CONSULTS_PER_RUN consults. The counter
//!   lives in the run context the tool loop creates per run and hands
//!   to run-aware tools at dispatch, so it resets with every new run.
//! - An unknown agent name returns an error the model can see, never
//!   an exception.
//!
//! The registry is resolved at call time, like the memory store, so
//! the server, the CLI, and tests can each pin their own with
//! `set_consult_registry()`. Mirrors `botropolis/tools/consult.py`.

use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};

use super::{builtin::arg_str, ToolCtx};
use crate::agent::RunOpts;
use crate::registry::AgentRegistry;
use crate::schemas::Task;

pub const MAX_CONSULTS_PER_RUN: u32 = 3;

static PINNED_REGISTRY: OnceLock<Mutex<Option<Arc<AgentRegistry>>>> = OnceLock::new();

fn pinned_slot() -> &'static Mutex<Option<Arc<AgentRegistry>>> {
    PINNED_REGISTRY.get_or_init(|| Mutex::new(None))
}

/// Pin the registry consult resolves agents from. None clears the
/// pin, so the next lookup builds the default registry.
pub fn set_consult_registry(registry: Option<Arc<AgentRegistry>>) {
    if let Ok(mut slot) = pinned_slot().lock() {
        *slot = registry;
    }
}

fn get_consult_registry(
    ctx_registry: Option<Arc<AgentRegistry>>,
) -> Result<Arc<AgentRegistry>, String> {
    if let Some(registry) = ctx_registry {
        return Ok(registry);
    }
    if let Ok(slot) = pinned_slot().lock() {
        if let Some(registry) = &*slot {
            return Ok(Arc::clone(registry));
        }
    }
    AgentRegistry::load()
        .map(Arc::new)
        .map_err(|e| format!("consult registry could not load: {e}"))
}

/// Ask another agent a question and get its answer back.
pub fn consult(args: &Value, ctx: &ToolCtx) -> Result<Value, String> {
    let target_name = arg_str(args, "agent")?.trim().to_string();
    let question = arg_str(args, "question")?.trim().to_string();
    if target_name.is_empty() {
        return Ok(json!({"error": "No agent named: the 'agent' argument was empty."}));
    }
    if question.is_empty() {
        return Ok(json!({"error": "Nothing to ask: the 'question' argument was empty."}));
    }
    let caller = ctx.agent_name.clone();
    if !caller.is_empty() && target_name.to_lowercase() == caller.to_lowercase() {
        return Ok(json!({"error": format!(
            "You cannot consult yourself ({caller}). Answer from your own knowledge, or ask a different specialist."
        )}));
    }
    let (depth, used) = {
        let context = ctx.run_context.borrow();
        (context.depth, context.consults_used)
    };
    if depth >= 1 {
        return Ok(
            json!({"error": "Consults do not chain: this run is itself a consultation, so it cannot consult another agent. Answer from your own knowledge."}),
        );
    }
    if used >= MAX_CONSULTS_PER_RUN {
        return Ok(json!({"error": format!(
            "Consult budget used up: this run already made {MAX_CONSULTS_PER_RUN} consults. Finish with what you have."
        )}));
    }
    let registry = match get_consult_registry(ctx.registry.clone()) {
        Ok(r) => r,
        Err(e) => return Ok(json!({"error": e})),
    };
    let target = match registry.get(&target_name) {
        Ok(agent) => agent,
        Err(_) => {
            return Ok(json!({"error": format!(
                "No agent named '{target_name}' on the roster. Check the spelling against the roster and try again."
            )}));
        }
    };
    ctx.run_context.borrow_mut().consults_used = used + 1;
    let mut task = Task::new(question);
    task.department = Some(target.department.clone());
    task.agent_name = Some(target.name.clone());
    let sub_ctx = std::rc::Rc::new(std::cell::RefCell::new(crate::tool_loop::RunContext {
        depth: depth + 1,
        consults_used: 0,
    }));
    let result = target.run(
        task,
        RunOpts {
            run_context: Some(sub_ctx),
            registry: Some(Arc::clone(&registry)),
            ..Default::default()
        },
    );
    if !result.success {
        return Ok(json!({
            "agent": target.name,
            "error": format!("{} could not complete the question: {}", target.name, result.error.unwrap_or_else(|| "run failed".to_string())),
        }));
    }
    Ok(json!({"agent": target.name, "answer": result.output}))
}
