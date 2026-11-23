"""The consult tool: ask a specialist agent a question mid-task.

Any agent with a toolkit can consult another agent: the named agent
runs the question as its own task and its answer comes back as the
tool result. This is how an agent gets a specialist's judgment
instead of guessing outside its field.

Guards, all enforced here rather than trusted to the model:

- An agent cannot consult itself.
- Depth is one: a consulted run runs with depth 1 in its run
  context, and a consult attempted at depth 1 is refused, so
  consults never chain.
- A run gets at most MAX_CONSULTS_PER_RUN consults. The counter
  lives in the run context the tool loop creates per run and hands
  to run-aware tools at dispatch (see execute_tool_call in
  botropolis/core/loop.py), so it resets with every new run.
- An unknown agent name returns an error the model can see, never
  an exception.

The registry is resolved at call time, like the memory store, so
the server, the CLI, and tests can each pin their own with
set_consult_registry(). The registry import is deferred for the
same reason: core.registry imports the tool package, so importing
it at module level would be a cycle.
"""

from __future__ import annotations

from typing import Any, Dict, Optional

from botropolis.tools.builtin import Tool

MAX_CONSULTS_PER_RUN = 3

_registry: Optional[Any] = None


def set_consult_registry(registry: Optional[Any]) -> None:
    """Pin the registry consult resolves agents from. None clears
    the pin, so the next lookup builds the default registry."""
    global _registry
    _registry = registry


def get_consult_registry() -> Any:
    """Return the pinned registry, building the default on first use."""
    global _registry
    if _registry is None:
        from botropolis.core.registry import AgentRegistry

        _registry = AgentRegistry()
    return _registry


class ConsultTool(Tool):
    """Ask another agent a question and get its answer back."""

    name = "consult"
    description = (
        "Ask a specialist agent one focused question and get their "
        "answer back. Use this when a task turns on another agent's "
        "field, security, data, research, cloud, instead of guessing "
        "outside your own. Name the agent exactly as it appears on "
        "the roster. You cannot consult yourself, a consulted agent "
        "cannot consult further, and you get at most three consults "
        "in one run, so make each question count."
    )
    parameters = {
        "agent": {
            "type": "string",
            "description": "The roster name of the agent to ask, e.g. 'auditor'",
            "required": True,
        },
        "question": {
            "type": "string",
            "description": "One focused question, with the facts the specialist needs",
            "required": True,
        },
    }

    # The loop fills `caller` (the asking agent's name) and
    # `run_context` (depth and consult budget for this run) in at
    # dispatch; neither is a model argument. `caller` is a separate
    # key from `agent` on purpose: `agent` here is the model's own
    # argument, the specialist being asked.
    caller_aware = True
    run_aware = True

    def execute(
        self,
        agent: str,
        question: str,
        caller: str = "",
        run_context: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        from botropolis.core.schemas import Task

        target_name = (agent or "").strip()
        question = (question or "").strip()
        if not target_name:
            return {"error": "No agent named: the 'agent' argument was empty."}
        if not question:
            return {"error": "Nothing to ask: the 'question' argument was empty."}
        if caller and target_name.lower() == caller.lower():
            return {
                "error": (
                    f"You cannot consult yourself ({caller}). Answer "
                    "from your own knowledge, or ask a different "
                    "specialist."
                )
            }
        context = run_context if run_context is not None else {}
        depth = int(context.get("depth", 0) or 0)
        if depth >= 1:
            return {
                "error": (
                    "Consults do not chain: this run is itself a "
                    "consultation, so it cannot consult another agent. "
                    "Answer from your own knowledge."
                )
            }
        used = int(context.get("consults_used", 0) or 0)
        if used >= MAX_CONSULTS_PER_RUN:
            return {
                "error": (
                    f"Consult budget used up: this run already made "
                    f"{MAX_CONSULTS_PER_RUN} consults. Finish with "
                    "what you have."
                )
            }
        try:
            target = get_consult_registry().get(target_name)
        except KeyError:
            return {
                "error": (
                    f"No agent named '{target_name}' on the roster. "
                    "Check the spelling against the roster and try "
                    "again."
                )
            }
        context["consults_used"] = used + 1
        result = target.run(
            Task(
                description=question,
                department=target.department,
                agent_name=target.name,
            ),
            run_context={"depth": depth + 1, "consults_used": 0},
        )
        if not result.success:
            return {
                "agent": target.name,
                "error": (
                    f"{target.name} could not complete the question: "
                    f"{result.error or 'run failed'}"
                ),
            }
        return {"agent": target.name, "answer": result.output}
