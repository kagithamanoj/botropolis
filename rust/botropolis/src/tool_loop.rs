//! Think-act-observe loop for tool-using agents.
//!
//! The protocol is plain text so it works with any model, including the
//! offline stub and small fine-tuned models. No provider-specific
//! function calling is needed. Mirrors `botropolis/core/loop.py`:
//!
//! The model replies with either:
//! ```text
//! ACTION: {"tool": "<name>", "args": {...}}
//! FINAL: <the answer>
//! ```
//! Anything else is treated as the final answer. After each action the
//! model receives an OBSERVATION: line with the tool result and
//! continues until it answers or runs out of steps.
//!
//! The loop has three exits besides answering: max steps, the same
//! tool call failing with the same error twice in a row, and token or
//! wall-clock budgets. Two habits are built in: plan first (a leading
//! PLAN: reply is recorded in the run state and the ledger) and verify
//! before done (one check pass, one revision, unparseable counts as a
//! pass).

use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::agent::{parse_model_reply, tool_prompt, Agent};
use crate::models::ChatMessage;
use crate::schemas::{Contract, Task, ToolCall};
use crate::tools::{self, ToolCtx};

pub const ACTION_PREFIX: &str = "ACTION:";
pub const FINAL_PREFIX: &str = "FINAL:";
pub const OBSERVATION_PREFIX: &str = "OBSERVATION:";
pub const PLAN_PREFIX: &str = "PLAN:";
pub const CHECK_PASS_MARKER: &str = "CHECK: PASS";
pub const PLAN_BEGIN_MESSAGE: &str = "Plan recorded. Begin: carry out the plan, starting with the first step. Reply with ACTION: or FINAL: as usual.";

/// Cap on raw tool results shown to the model.
pub const OBSERVATION_CHARS: usize = 8000;

/// Per-run shared context, handed to run-aware tools at dispatch.
/// A consult sub-run arrives with its own context (depth already
/// raised), which is how consults cannot chain.
#[derive(Debug, Clone, Default)]
pub struct RunContext {
    pub depth: u32,
    pub consults_used: u32,
}

/// One snapshot of a run's progress, handed to the state hook after
/// every step so run state (and the task map) can be written.
#[derive(Debug, Clone)]
pub struct LoopStateSnapshot {
    pub run_id: Option<String>,
    pub agent: String,
    pub task: Task,
    pub contract: Option<Contract>,
    pub steps: Vec<ToolCall>,
    pub plan: Option<String>,
    pub status: String,
    pub answer: Option<String>,
    pub reason: Option<String>,
}

impl LoopStateSnapshot {
    /// The state dict, with the same keys the Python RunStore writes.
    pub fn to_json(&self) -> Value {
        json!({
            "run_id": self.run_id,
            "agent": self.agent,
            "task": self.task,
            "contract": self.contract,
            "steps": self.steps,
            "plan": self.plan,
            "status": self.status,
            "answer": self.answer,
            "reason": self.reason,
        })
    }
}

/// Usage summed across a loop's model calls.
#[derive(Debug, Clone, Default)]
pub struct LoopUsage {
    pub input: u64,
    pub output: u64,
    pub escalated: bool,
    pub escalation_reason: Option<String>,
    pub answer_checked: bool,
    pub check_revised: bool,
}

/// A loop event callback: `on_event(kind, payload)`.
pub type EventCallback = Box<dyn FnMut(&str, Value) + Send>;
/// A run-state snapshot callback, called after every step.
pub type StateHook = Box<dyn FnMut(&LoopStateSnapshot) + Send>;

/// Options for one `run_tool_loop` call.
#[derive(Default)]
pub struct LoopOpts {
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<f64>,
    pub run_id: Option<String>,
    /// A saved state dict to resume from, if any.
    pub resume_state: Option<Value>,
    /// The run's task map in words, read back on resume.
    pub map_text: Option<String>,
    pub run_context: Option<Rc<RefCell<RunContext>>>,
    pub registry: Option<Arc<crate::registry::AgentRegistry>>,
    /// Called as `on_event(kind, payload)` with tool_started,
    /// tool_finished, tool_blocked, plan_recorded, and answer_checked.
    /// A raising callback never breaks the loop.
    pub on_event: Option<EventCallback>,
    /// Called with a snapshot after every step, when run state is
    /// being written.
    pub state_hook: Option<StateHook>,
}

/// Return the plan text when a reply is a plan, else None.
///
/// A plan is a reply whose first non-empty line starts with "PLAN:".
/// The whole reply is the plan; there is no other content to parse.
pub fn parse_plan_reply(text: &str) -> Option<String> {
    let stripped = text.trim();
    if stripped.is_empty() {
        return None;
    }
    let first_line = stripped.lines().next().unwrap_or("").trim();
    if first_line.starts_with(PLAN_PREFIX) {
        Some(stripped.to_string())
    } else {
        None
    }
}

/// The verification message shown to the model before an answer is accepted.
pub fn check_prompt(task_description: &str, draft: &str) -> String {
    format!(
        "Before this run ends, check your draft answer against the task. Look for errors, requirements the draft does not meet, and claims your observations do not support.\n\nTask: {task_description}\n\nDraft answer:\n{draft}\n\nIf the answer is sound, reply with exactly: {CHECK_PASS_MARKER}\nIf it needs fixing, reply with FINAL: followed by the full corrected answer. Only one revision is accepted."
    )
}

/// Read a check reply. Returns (accepted_answer, revised).
///
/// A pass marker accepts the draft. A FINAL: line, on its own or after
/// a CHECK: line, is the revised answer, unless it restates the draft
/// word for word, which is a pass in effect. Anything else cannot be
/// parsed and counts as a pass: the check must never break a run that
/// already produced an answer.
pub fn parse_check_reply(text: &str, draft: &str) -> (String, bool) {
    let stripped = text.trim();
    if stripped.is_empty() {
        return (draft.to_string(), false);
    }
    let first_line = stripped.lines().next().unwrap_or("").trim().to_uppercase();
    if first_line.starts_with("CHECK:") && first_line.contains("PASS") {
        return (draft.to_string(), false);
    }
    if let Some(rest) = stripped.strip_prefix(FINAL_PREFIX) {
        let revised = rest.trim();
        if !revised.is_empty() && revised != draft {
            return (revised.to_string(), true);
        }
        return (draft.to_string(), false);
    }
    if first_line.starts_with("CHECK:") {
        if let Some(idx) = stripped.find(FINAL_PREFIX) {
            let revised = stripped[idx + FINAL_PREFIX.len()..].trim();
            if !revised.is_empty() && revised != draft {
                return (revised.to_string(), true);
            }
        }
        return (draft.to_string(), false);
    }
    (draft.to_string(), false)
}

fn preview(result: &Value) -> String {
    let text = match result {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    };
    let text = text.trim().to_string();
    if text.chars().count() > OBSERVATION_CHARS {
        let truncated: String = text.chars().take(OBSERVATION_CHARS).collect();
        format!("{truncated}... [truncated]")
    } else {
        text
    }
}

/// Run one tool call, never raising. Failures become failed ToolCalls.
///
/// Agent-aware tools receive the calling agent's name, caller-aware
/// tools receive it under "caller", and run-aware tools receive the
/// run's shared context. The recorded ToolCall keeps the model's own
/// args; the injected names are an execution detail, not something
/// the model said.
pub fn execute_tool_call(
    toolkit: &[String],
    tool_name: &str,
    args: &Value,
    ctx: &ToolCtx,
) -> ToolCall {
    let started = Instant::now();
    let elapsed = || started.elapsed().as_secs_f64() * 1000.0;
    if !toolkit.contains(&tool_name.to_string()) {
        return ToolCall {
            tool: tool_name.to_string(),
            args: args.clone(),
            success: false,
            result_preview: format!("Unknown tool '{tool_name}'. Available: {:?}", {
                let mut sorted = toolkit.to_vec();
                sorted.sort();
                sorted
            }),
            elapsed_ms: elapsed(),
        };
    }
    let def = match tools::get_tool_def(tool_name) {
        Some(d) => d,
        None => {
            return ToolCall {
                tool: tool_name.to_string(),
                args: args.clone(),
                success: false,
                result_preview: format!("Unknown tool '{tool_name}'"),
                elapsed_ms: elapsed(),
            }
        }
    };
    let mut exec_args = args.clone();
    if def.agent_aware {
        if let Value::Object(map) = &mut exec_args {
            map.insert("agent".to_string(), Value::String(ctx.agent_name.clone()));
        }
    }
    if def.caller_aware {
        if let Value::Object(map) = &mut exec_args {
            map.insert("caller".to_string(), Value::String(ctx.agent_name.clone()));
        }
    }
    match tools::dispatch(tool_name, &exec_args, ctx) {
        Ok(result) => ToolCall {
            tool: tool_name.to_string(),
            args: args.clone(),
            success: true,
            result_preview: preview(&result),
            elapsed_ms: elapsed(),
        },
        Err(e) => ToolCall {
            tool: tool_name.to_string(),
            args: args.clone(),
            success: false,
            result_preview: e,
            elapsed_ms: elapsed(),
        },
    }
}

/// Write one run-state snapshot through the state hook. Never raises.
#[allow(clippy::too_many_arguments)]
fn save_state(
    hook: &mut Option<StateHook>,
    run_id: &Option<String>,
    agent_name: &str,
    task: &Task,
    steps: &[ToolCall],
    plan: &Option<String>,
    status: &str,
    answer: Option<String>,
    reason: Option<String>,
) {
    if let Some(hook) = hook.as_mut() {
        let snapshot = LoopStateSnapshot {
            run_id: run_id.clone(),
            agent: agent_name.to_string(),
            task: task.clone(),
            contract: task.contract.clone(),
            steps: steps.to_vec(),
            plan: plan.clone(),
            status: status.to_string(),
            answer,
            reason,
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(&snapshot)));
    }
}

/// Run the think-act-observe loop for one task.
///
/// Returns (final_answer, tool_calls, usage). On an escalated exit
/// usage carries escalated=true and the reason. A model client error
/// on the main loop call propagates as Err; the check call failing
/// never does, the draft stands.
pub fn run_tool_loop(
    agent: &Agent,
    task: &Task,
    opts: &mut LoopOpts,
) -> Result<(String, Vec<ToolCall>, LoopUsage), String> {
    let token_budget = opts.token_budget.or(agent.token_budget);
    let time_budget_seconds = opts.time_budget_seconds.or(agent.time_budget_seconds);
    let run_context = opts
        .run_context
        .clone()
        .unwrap_or_else(|| Rc::new(RefCell::new(RunContext::default())));

    let messages = agent.build_messages(task);
    let system = format!("{}\n\n{}", messages[0].content, tool_prompt(&agent.toolkit));
    let mut convo: Vec<ChatMessage> = vec![ChatMessage::new("system", system)];
    convo.extend(messages[1..].iter().cloned());

    // Resume: rebuild the tool calls and the conversation from the
    // steps a previous, interrupted run already completed.
    let mut prior_steps: Vec<Value> = Vec::new();
    let mut plan_text: Option<String> = None;
    if let Some(state) = &opts.resume_state {
        if let Some(steps) = state.get("steps").and_then(|s| s.as_array()) {
            prior_steps = steps.clone();
        }
        if let Some(plan) = state.get("plan").and_then(|p| p.as_str()) {
            if !plan.trim().is_empty() {
                plan_text = Some(plan.to_string());
            }
        }
    }
    let first_reply = plan_text.is_none() && prior_steps.is_empty();
    let mut first_reply = first_reply;
    let mut tool_calls: Vec<ToolCall> = prior_steps
        .iter()
        .map(|step| ToolCall {
            tool: step
                .get("tool")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            args: step.get("args").cloned().unwrap_or(Value::Null),
            success: step
                .get("success")
                .and_then(|s| s.as_bool())
                .unwrap_or(true),
            result_preview: step
                .get("result_preview")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string(),
            elapsed_ms: step
                .get("elapsed_ms")
                .and_then(|e| e.as_f64())
                .unwrap_or(0.0),
        })
        .collect();
    if let Some(plan) = &plan_text {
        convo.push(ChatMessage::new("assistant", plan.clone()));
        convo.push(ChatMessage::new("user", PLAN_BEGIN_MESSAGE.to_string()));
    }
    for step in &prior_steps {
        let action = json!({
            "tool": step.get("tool"),
            "args": step.get("args").cloned().unwrap_or(Value::Null),
        });
        convo.push(ChatMessage::new(
            "assistant",
            format!(
                "{ACTION_PREFIX} {}",
                serde_json::to_string(&action).unwrap_or_default()
            ),
        ));
        let success = step
            .get("success")
            .and_then(|s| s.as_bool())
            .unwrap_or(true);
        let observation = if success {
            step.get("result_preview")
                .and_then(|r| r.as_str())
                .unwrap_or("(empty result)")
                .to_string()
        } else {
            format!(
                "Tool call failed: {}",
                step.get("result_preview")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
            )
        };
        convo.push(ChatMessage::new(
            "user",
            format!("{OBSERVATION_PREFIX} {observation}"),
        ));
    }
    if opts.resume_state.is_some() {
        if let Some(map_text) = &opts.map_text {
            if !map_text.is_empty() {
                convo.push(ChatMessage::new(
                    "user",
                    format!("Task map so far:\n{map_text}"),
                ));
            }
        }
    }

    let mut usage = LoopUsage::default();
    let mut last_text = String::new();
    let max_steps = agent.max_steps.max(1) as usize;
    let loop_started = Instant::now();
    let mut last_failure: Option<(String, String, String)> = None;
    let mut failure_streak = 0usize;

    let over_time_budget = |loop_started: Instant| -> bool {
        match time_budget_seconds {
            Some(budget) => loop_started.elapsed().as_secs_f64() > budget,
            None => false,
        }
    };

    if prior_steps.len() >= max_steps {
        let reason = format!(
            "run already used all {max_steps} steps before it was interrupted; nothing left to resume with"
        );
        usage.escalated = true;
        usage.escalation_reason = Some(reason.clone());
        save_state(
            &mut opts.state_hook,
            &opts.run_id,
            &agent.name,
            task,
            &tool_calls,
            &plan_text,
            "escalated",
            None,
            Some(reason.clone()),
        );
        return Ok((format!("Escalated: {reason}"), tool_calls, usage));
    }

    let steps_left = max_steps - prior_steps.len();
    for _ in 0..steps_left {
        if over_time_budget(loop_started) {
            let reason = format!(
                "time budget exceeded: {:.1}s elapsed, budget is {}s",
                loop_started.elapsed().as_secs_f64(),
                time_budget_seconds.unwrap_or(0.0)
            );
            usage.escalated = true;
            usage.escalation_reason = Some(reason.clone());
            save_state(
                &mut opts.state_hook,
                &opts.run_id,
                &agent.name,
                task,
                &tool_calls,
                &plan_text,
                "escalated",
                None,
                Some(reason.clone()),
            );
            return Ok((format!("Escalated: {reason}"), tool_calls, usage));
        }
        let text = agent
            .client()
            .chat(&agent.model, &convo)
            .map_err(|e| e.to_string())?;
        last_text = text.clone();
        let reported = agent.client().last_usage();
        usage.input += reported.input;
        usage.output += reported.output;

        if let Some(budget) = token_budget {
            let used = usage.input + usage.output;
            if used > budget {
                let reason =
                    format!("token budget exceeded: {used} tokens used, budget is {budget}");
                usage.escalated = true;
                usage.escalation_reason = Some(reason.clone());
                save_state(
                    &mut opts.state_hook,
                    &opts.run_id,
                    &agent.name,
                    task,
                    &tool_calls,
                    &plan_text,
                    "escalated",
                    None,
                    Some(reason.clone()),
                );
                return Ok((format!("Escalated: {reason}"), tool_calls, usage));
            }
        }
        if over_time_budget(loop_started) {
            let reason = format!(
                "time budget exceeded: {:.1}s elapsed, budget is {}s",
                loop_started.elapsed().as_secs_f64(),
                time_budget_seconds.unwrap_or(0.0)
            );
            usage.escalated = true;
            usage.escalation_reason = Some(reason.clone());
            save_state(
                &mut opts.state_hook,
                &opts.run_id,
                &agent.name,
                task,
                &tool_calls,
                &plan_text,
                "escalated",
                None,
                Some(reason.clone()),
            );
            return Ok((format!("Escalated: {reason}"), tool_calls, usage));
        }

        if let Some(plan_reply) = parse_plan_reply(&text) {
            if plan_text.is_none() && first_reply {
                plan_text = Some(plan_reply.clone());
                first_reply = false;
                emit(
                    &mut opts.on_event,
                    "plan_recorded",
                    json!({"plan": plan_reply}),
                );
                convo.push(ChatMessage::new("assistant", text.clone()));
                convo.push(ChatMessage::new("user", PLAN_BEGIN_MESSAGE.to_string()));
                save_state(
                    &mut opts.state_hook,
                    &opts.run_id,
                    &agent.name,
                    task,
                    &tool_calls,
                    &plan_text,
                    "running",
                    None,
                    None,
                );
                continue;
            }
            first_reply = false;
            convo.push(ChatMessage::new("assistant", text.clone()));
            convo.push(ChatMessage::new(
                "user",
                format!(
                    "{OBSERVATION_PREFIX} A plan only counts as the first reply of a run, and this run is past that point. Continue with ACTION: or FINAL:."
                ),
            ));
            continue;
        }
        first_reply = false;

        let (kind, payload) = parse_model_reply(&text);
        if kind == "final" {
            let draft = payload;
            let mut answer = draft.clone();
            let mut revised = false;
            let mut check_convo = convo.clone();
            check_convo.push(ChatMessage::new("assistant", text.clone()));
            check_convo.push(ChatMessage::new(
                "user",
                check_prompt(&task.description, &draft),
            ));
            if let Ok(check_text) = agent.client().chat(&agent.model, &check_convo) {
                let reported = agent.client().last_usage();
                usage.input += reported.input;
                usage.output += reported.output;
                let (accepted, was_revised) = parse_check_reply(&check_text, &draft);
                answer = accepted;
                revised = was_revised;
            }
            usage.answer_checked = true;
            usage.check_revised = revised;
            emit(
                &mut opts.on_event,
                "answer_checked",
                json!({"revised": revised}),
            );
            save_state(
                &mut opts.state_hook,
                &opts.run_id,
                &agent.name,
                task,
                &tool_calls,
                &plan_text,
                "finished",
                Some(answer.clone()),
                None,
            );
            return Ok((answer, tool_calls, usage));
        }

        convo.push(ChatMessage::new("assistant", text.clone()));
        if kind == "malformed" {
            let observation = format!(
                "Could not parse your ACTION line ({payload}). Reply with ACTION: {{\"tool\": \"<name>\", \"args\": {{...}}}} or FINAL: <answer>."
            );
            convo.push(ChatMessage::new(
                "user",
                format!("{OBSERVATION_PREFIX} {observation}"),
            ));
            continue;
        }

        // kind == "action": payload is the raw JSON after ACTION:.
        let parsed: Value = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(_) => {
                convo.push(ChatMessage::new(
                    "user",
                    format!("{OBSERVATION_PREFIX} Could not parse your ACTION line. Reply with ACTION: {{\"tool\": \"<name>\", \"args\": {{...}}}} or FINAL: <answer>."),
                ));
                continue;
            }
        };
        let tool_name = parsed
            .get("tool")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let args = parsed
            .get("args")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        emit(
            &mut opts.on_event,
            "tool_started",
            json!({"tool": tool_name, "args": args}),
        );
        let blocked = task.needs_approval.contains(&tool_name) && !task.approved;
        let ctx = ToolCtx {
            agent_name: agent.name.clone(),
            run_context: Rc::clone(&run_context),
            registry: opts.registry.clone(),
        };
        let call = if blocked {
            let call = ToolCall {
                tool: tool_name.clone(),
                args: args.clone(),
                success: false,
                result_preview: format!(
                    "Tool '{tool_name}' needs human approval and this run is not approved. The call did not run."
                ),
                elapsed_ms: 0.0,
            };
            emit(
                &mut opts.on_event,
                "tool_blocked",
                serde_json::to_value(&call).unwrap_or(Value::Null),
            );
            call
        } else {
            execute_tool_call(&agent.toolkit, &tool_name, &args, &ctx)
        };
        tool_calls.push(call.clone());
        emit(
            &mut opts.on_event,
            "tool_finished",
            serde_json::to_value(&call).unwrap_or(Value::Null),
        );
        save_state(
            &mut opts.state_hook,
            &opts.run_id,
            &agent.name,
            task,
            &tool_calls,
            &plan_text,
            "running",
            None,
            None,
        );
        let observation = if call.success {
            last_failure = None;
            failure_streak = 0;
            if call.result_preview.is_empty() {
                "(empty result)".to_string()
            } else {
                call.result_preview.clone()
            }
        } else if blocked {
            format!(
                "Tool call blocked: {} Do not retry it. Continue without it, or finish with what you have.",
                call.result_preview
            )
        } else {
            let observation = format!("Tool call failed: {}", call.result_preview);
            let signature = (
                call.tool.clone(),
                serde_json::to_string(&sorted_json(&call.args)).unwrap_or_default(),
                call.result_preview.clone(),
            );
            if Some(&signature) == last_failure.as_ref() {
                failure_streak += 1;
            } else {
                last_failure = Some(signature);
                failure_streak = 1;
            }
            if failure_streak >= 2 {
                let reason = format!(
                    "tool '{}' failed twice in a row with the same error: {}",
                    call.tool, call.result_preview
                );
                usage.escalated = true;
                usage.escalation_reason = Some(reason.clone());
                save_state(
                    &mut opts.state_hook,
                    &opts.run_id,
                    &agent.name,
                    task,
                    &tool_calls,
                    &plan_text,
                    "escalated",
                    None,
                    Some(reason.clone()),
                );
                return Ok((format!("Escalated: {reason}"), tool_calls, usage));
            }
            observation
        };
        convo.push(ChatMessage::new(
            "user",
            format!("{OBSERVATION_PREFIX} {observation}"),
        ));
    }

    // Out of steps: return the last thing the model said.
    save_state(
        &mut opts.state_hook,
        &opts.run_id,
        &agent.name,
        task,
        &tool_calls,
        &plan_text,
        "finished",
        Some(last_text.clone()),
        None,
    );
    Ok((last_text, tool_calls, usage))
}

fn emit(on_event: &mut Option<EventCallback>, kind: &str, payload: Value) {
    if let Some(cb) = on_event.as_mut() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(kind, payload)));
    }
}

/// A JSON value with object keys sorted, for same-error signatures.
fn sorted_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for key in keys {
                out.insert(key.clone(), sorted_json(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_json).collect()),
        other => other.clone(),
    }
}
