"""Think-act-observe loop for tool-using agents.

The protocol is plain text so it works with any model, including the
offline stub and small fine-tuned models. No provider-specific function
calling is needed.

The model replies with either:
    ACTION: {"tool": "<name>", "args": {...}}
    FINAL: <the answer>

Anything else is treated as the final answer. After each action the
model receives an OBSERVATION: line with the tool result and continues
until it answers or runs out of steps.

The loop has three exits besides answering:

- max_steps, the original cap on model calls.
- Same-error exit: the same tool call failing with the same error
  twice in a row stops the loop. Retrying an identical failure burns
  budget without new information, so the run escalates instead.
- Budgets: an optional token budget and wall-clock budget per run
  (agent spec or run parameter). Going over either stops the run
  cleanly with an escalated result naming the budget that was hit.

Escalated exits return usage["escalated"] = True and
usage["escalation_reason"], and the answer text says what happened.
"""

from __future__ import annotations

import json
import time
from typing import TYPE_CHECKING, Any, Dict, List, Optional, Tuple

from botropolis.core.schemas import ToolCall
from botropolis.tools import get_tool

if TYPE_CHECKING:
    from botropolis.core.agent import Agent
    from botropolis.core.runstate import RunStore
    from botropolis.core.schemas import Task

ACTION_PREFIX = "ACTION:"
FINAL_PREFIX = "FINAL:"
OBSERVATION_PREFIX = "OBSERVATION:"

# Default cap on loop iterations. Each step is one model call plus at
# most one tool execution, so this bounds both cost and runtime.
DEFAULT_MAX_STEPS = 8

# Tool results can be long; the model only needs the head of them.
OBSERVATION_CHARS = 2000


def tool_prompt(tool_names: List[str]) -> str:
    """Build the tool-use instructions appended to the system prompt."""
    lines = ["You have these tools. Use them when they help.", ""]
    for name in tool_names:
        tool = get_tool(name)
        params = ", ".join(
            f"{key} ({spec.get('type', '?')})"
            for key, spec in tool.parameters.items()
        )
        lines.append(f"- {tool.name}({params}): {tool.description}")
    lines += [
        "",
        "Reply with exactly one of these:",
        'ACTION: {"tool": "<name>", "args": {...}}',
        "FINAL: <your answer>",
        "",
        "After each ACTION you receive an OBSERVATION: line with the "
        "result. Use observations to decide the next step. When you have "
        "what you need, reply with FINAL: and your answer.",
    ]
    return "\n".join(lines)


def parse_model_reply(text: str) -> Tuple[str, Any]:
    """Parse one model reply into (kind, payload).

    Kinds: "action" -> (tool_name, args), "final" -> answer text,
    "malformed" -> error description for the ACTION line.
    """
    stripped = text.strip()
    if stripped.startswith(FINAL_PREFIX):
        return ("final", stripped[len(FINAL_PREFIX):].strip())
    if stripped.startswith(ACTION_PREFIX):
        raw = stripped[len(ACTION_PREFIX):].strip()
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError as exc:
            return ("malformed", f"invalid JSON: {exc}")
        if not isinstance(payload, dict) or "tool" not in payload:
            return ("malformed", 'ACTION must be {"tool": "<name>", "args": {...}}')
        args = payload.get("args", {})
        if not isinstance(args, dict):
            return ("malformed", '"args" must be an object')
        tool_name = payload["tool"]
        if not isinstance(tool_name, str):
            return ("malformed", '"tool" must be a string')
        return ("action", (tool_name, args))
    # Plain prose counts as the answer. Small models and the offline stub
    # will usually land here, which keeps the loop safe by default.
    return ("final", text)


def _preview(result: Any) -> str:
    """Render a tool result as a short string for observations and logs."""
    if isinstance(result, str):
        text = result
    else:
        try:
            text = json.dumps(result, default=str)
        except (TypeError, ValueError):
            text = str(result)
    text = text.strip()
    if len(text) > OBSERVATION_CHARS:
        text = text[:OBSERVATION_CHARS] + "... [truncated]"
    return text


def execute_tool_call(
    toolkit: List[str], tool_name: str, args: Dict[str, Any]
) -> ToolCall:
    """Run one tool call, never raising. Failures become failed ToolCalls."""
    started = time.time()
    elapsed = lambda: (time.time() - started) * 1000.0
    if tool_name not in toolkit:
        return ToolCall(
            tool=tool_name,
            args=args,
            success=False,
            result_preview=(
                f"Unknown tool '{tool_name}'. Available: {sorted(toolkit)}"
            ),
            elapsed_ms=elapsed(),
        )
    try:
        result = get_tool(tool_name).execute(**args)
    except Exception as exc:
        return ToolCall(
            tool=tool_name,
            args=args,
            success=False,
            result_preview=f"{type(exc).__name__}: {exc}",
            elapsed_ms=elapsed(),
        )
    return ToolCall(
        tool=tool_name,
        args=args,
        success=True,
        result_preview=_preview(result),
        elapsed_ms=elapsed(),
    )


def run_tool_loop(
    agent: "Agent",
    task: "Task",
    on_event=None,
    token_budget: Optional[int] = None,
    time_budget_seconds: Optional[float] = None,
    store: Optional["RunStore"] = None,
    run_id: Optional[str] = None,
    resume_state: Optional[Dict[str, Any]] = None,
) -> Tuple[str, List[ToolCall], Dict[str, Any]]:
    """Run the think-act-observe loop for one task.

    Returns (final_answer, tool_calls, usage) where usage is
    {"input": n, "output": n} summed across the loop's model calls.
    On an escalated exit (same-error or budget) usage also carries
    "escalated": True and "escalation_reason": str.

    token_budget and time_budget_seconds default to the agent's spec
    values when not passed; passing a value here overrides the spec
    for this run only.

    When store and run_id are given, run state is written after every
    step (see botropolis/core/runstate.py). resume_state, a state dict
    loaded from that store, seeds the conversation with the completed
    steps so the run continues where it stopped instead of redoing
    finished tool calls. Prior steps count against max_steps: each one
    already used a model call.

    on_event, when given, is called as on_event(kind, payload) with
    ("tool_started", {"tool", "args"}) before each tool runs and
    ("tool_finished", tool_call_dict) after. It never breaks the loop:
    a raising callback is swallowed.
    """

    def emit(kind: str, payload: Dict[str, Any]) -> None:
        if on_event is None:
            return
        try:
            on_event(kind, payload)
        except Exception:
            pass

    if token_budget is None:
        token_budget = getattr(agent, "token_budget", None)
    if time_budget_seconds is None:
        time_budget_seconds = getattr(agent, "time_budget_seconds", None)

    messages = agent.build_messages(task)
    system = messages[0]["content"] + "\n\n" + tool_prompt(agent.toolkit)
    convo = [{"role": "system", "content": system}] + messages[1:]

    # Resume: rebuild the tool calls and the conversation from the
    # steps a previous, interrupted run already completed.
    prior_steps: List[Dict[str, Any]] = []
    if resume_state:
        prior_steps = list(resume_state.get("steps") or [])
    tool_calls: List[ToolCall] = [
        ToolCall(
            tool=str(step.get("tool", "")),
            args=dict(step.get("args") or {}),
            success=bool(step.get("success", True)),
            result_preview=str(step.get("result_preview", "")),
            elapsed_ms=float(step.get("elapsed_ms", 0.0) or 0.0),
        )
        for step in prior_steps
    ]
    for step in prior_steps:
        action = json.dumps(
            {"tool": step.get("tool"), "args": step.get("args") or {}}
        )
        convo.append({"role": "assistant", "content": f"{ACTION_PREFIX} {action}"})
        if step.get("success", True):
            observation = step.get("result_preview") or "(empty result)"
        else:
            observation = f"Tool call failed: {step.get('result_preview', '')}"
        convo.append(
            {"role": "user", "content": f"{OBSERVATION_PREFIX} {observation}"}
        )

    usage: Dict[str, Any] = {"input": 0, "output": 0}
    last_text = ""
    max_steps = max(1, agent.max_steps)
    loop_started = time.time()
    last_failure: Optional[Tuple[str, str, str]] = None
    failure_streak = 0

    def save_state(
        status: str,
        answer: Optional[str] = None,
        reason: Optional[str] = None,
    ) -> None:
        if store is None or run_id is None:
            return
        try:
            store.save(
                {
                    "run_id": run_id,
                    "agent": agent.name,
                    "task": task.to_dict(),
                    "contract": (
                        task.contract.to_dict() if task.contract else None
                    ),
                    "steps": [call.to_dict() for call in tool_calls],
                    "status": status,
                    "answer": answer,
                    "reason": reason,
                }
            )
        except Exception:
            pass

    def escalate(reason: str) -> Tuple[str, List[ToolCall], Dict[str, Any]]:
        usage["escalated"] = True
        usage["escalation_reason"] = reason
        save_state("escalated", reason=reason)
        return f"Escalated: {reason}", tool_calls, usage

    def over_time_budget() -> bool:
        return (
            time_budget_seconds is not None
            and time.time() - loop_started > time_budget_seconds
        )

    if prior_steps and len(prior_steps) >= max_steps:
        return escalate(
            f"run already used all {max_steps} steps before it was "
            "interrupted; nothing left to resume with"
        )

    for _ in range(max_steps - len(prior_steps)):
        if over_time_budget():
            return escalate(
                f"time budget exceeded: {time.time() - loop_started:.1f}s "
                f"elapsed, budget is {time_budget_seconds}s"
            )
        text = agent.client.chat(agent.model, convo)
        last_text = text
        reported = getattr(agent.client, "last_usage", None) or {}
        usage["input"] += int(reported.get("input", 0) or 0)
        usage["output"] += int(reported.get("output", 0) or 0)

        if token_budget is not None:
            used = usage["input"] + usage["output"]
            if used > token_budget:
                return escalate(
                    f"token budget exceeded: {used} tokens used, "
                    f"budget is {token_budget}"
                )
        if over_time_budget():
            return escalate(
                f"time budget exceeded: {time.time() - loop_started:.1f}s "
                f"elapsed, budget is {time_budget_seconds}s"
            )

        kind, payload = parse_model_reply(text)
        if kind == "final":
            save_state("finished", answer=payload)
            return payload, tool_calls, usage

        convo.append({"role": "assistant", "content": text})
        if kind == "malformed":
            observation = (
                f"Could not parse your ACTION line ({payload}). Reply with "
                'ACTION: {"tool": "<name>", "args": {...}} or FINAL: <answer>.'
            )
        else:
            tool_name, args = payload
            emit("tool_started", {"tool": tool_name, "args": args})
            call = execute_tool_call(agent.toolkit, tool_name, args)
            tool_calls.append(call)
            emit("tool_finished", call.to_dict())
            save_state("running")
            if call.success:
                last_failure = None
                failure_streak = 0
                observation = call.result_preview or "(empty result)"
            else:
                observation = f"Tool call failed: {call.result_preview}"
                signature = (
                    call.tool,
                    json.dumps(call.args, sort_keys=True, default=str),
                    call.result_preview,
                )
                if signature == last_failure:
                    failure_streak += 1
                else:
                    last_failure = signature
                    failure_streak = 1
                if failure_streak >= 2:
                    return escalate(
                        f"tool '{call.tool}' failed twice in a row with "
                        f"the same error: {call.result_preview}"
                    )
        convo.append({"role": "user", "content": f"{OBSERVATION_PREFIX} {observation}"})

    # Out of steps: return the last thing the model said. The tool calls
    # made so far are still recorded on the result.
    save_state("finished", answer=last_text)
    return last_text, tool_calls, usage
