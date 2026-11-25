"""A human-readable task map for one run.

Run state JSON and the ledger are the machine record of a run. The
task map is the readable one: a short markdown file written next to
the run state at botropolis/data/runs/<run_id>.map.md (gitignored,
same as run state). It says, in words, where the run stands: the
goal and what counts as done, the recorded plan with each step marked
done or pending, the tool calls made so far with one-line outcomes,
and any blockers.

The plan marks are a progress sketch, not a proof. Plan steps are
marked done in order as tool calls succeed: the first successful
call completes the first step, the next the second, and so on. A
plan step that needs no tool stays pending until the run finishes.
The map never claims a step is done because the model said so; it
counts what actually ran.

A resumed run reads its map back and the loop attaches it to the
conversation, so the continuing agent sees the work in words, not
just raw steps.

Writing a map never breaks a run: a failed write is swallowed, the
same as a failed run-state write.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Dict, List

from botropolis.core.runstate import RunStore

MAP_SUFFIX = ".map.md"


def plan_steps(plan_text: str | None) -> List[str]:
    """Split a recorded plan into its steps, in order.

    The first line carries the PLAN: prefix; the steps are the lines
    after it, with list markers and numbering stripped. Empty lines
    drop out. A plan that is one line with no steps after it yields
    that line as its single step.
    """
    if not plan_text:
        return []
    lines = [line.strip() for line in str(plan_text).strip().splitlines()]
    if not lines:
        return []
    first = lines[0]
    if first.upper().startswith("PLAN:"):
        first = first[len("PLAN:"):].strip()
    rest = [first] if first else []
    rest.extend(lines[1:])
    steps = []
    for line in rest:
        step = re.sub(r"^(\d+[.)]|[-*])\s+", "", line).strip()
        if step:
            steps.append(step)
    return steps


def _one_line(text: str, limit: int = 120) -> str:
    """Collapse a result preview to a single short line."""
    flat = " ".join(str(text or "").split())
    if len(flat) > limit:
        return flat[:limit] + "..."
    return flat or "(empty result)"


def render_map(state: Dict[str, Any]) -> str:
    """Render a run state dict as a task map in markdown."""
    run_id = str(state.get("run_id", ""))
    task = state.get("task") or {}
    contract = state.get("contract") or task.get("contract") or {}
    steps = list(state.get("steps") or [])
    status = str(state.get("status", "running"))
    lines = [f"# Task map: {run_id}", ""]
    lines.append(f"Agent: {state.get('agent', '')}")
    lines.append(f"Task: {task.get('description', '')}")
    lines.append(f"Status: {status}")
    if state.get("reason"):
        lines.append(f"Reason: {state['reason']}")
    lines.append("")
    goal = contract.get("goal") if isinstance(contract, dict) else None
    if goal:
        lines.append("## Goal")
        lines.append(str(goal))
        lines.append("")
    done_when = contract.get("done_when") if isinstance(contract, dict) else None
    if done_when:
        lines.append("## Done when")
        for item in done_when:
            lines.append(f"- {item}")
        lines.append("")
    plan = plan_steps(state.get("plan"))
    if plan:
        done_count = min(
            sum(1 for step in steps if step.get("success", True)), len(plan)
        )
        lines.append("## Plan")
        for index, step in enumerate(plan):
            mark = "x" if index < done_count else " "
            lines.append(f"- [{mark}] {step}")
        lines.append("")
    lines.append("## Tool calls")
    if not steps:
        lines.append("- none yet")
    for step in steps:
        outcome = "ok" if step.get("success", True) else "FAILED"
        lines.append(
            f"- {step.get('tool', '')} {_one_line(step.get('args'))} "
            f"-> {outcome}: {_one_line(step.get('result_preview', ''))}"
        )
    lines.append("")
    blockers = [
        step
        for step in steps
        if not step.get("success", True)
    ]
    if blockers or status == "escalated":
        lines.append("## Blockers")
        for step in blockers:
            lines.append(
                f"- {step.get('tool', '')} failed: "
                f"{_one_line(step.get('result_preview', ''))}"
            )
        if status == "escalated" and state.get("reason"):
            lines.append(f"- escalated: {state['reason']}")
        lines.append("")
    answer = state.get("answer")
    if answer:
        lines.append("## Answer")
        lines.append(str(answer).strip())
        lines.append("")
    return "\n".join(lines).strip() + "\n"


def map_path(store: RunStore, run_id: str) -> Path:
    """Where one run's map file lives, next to its state file."""
    return store.root / f"{run_id}{MAP_SUFFIX}"


def write_map(store: RunStore, state: Dict[str, Any]) -> None:
    """Write one run's map from its state dict. Never raises."""
    try:
        store.root.mkdir(parents=True, exist_ok=True)
        map_path(store, str(state["run_id"])).write_text(
            render_map(state), encoding="utf-8"
        )
    except Exception:
        pass


def read_map(store: RunStore, run_id: str) -> str:
    """Read one run's map. Falls back to rendering from state when
    the map file is missing (older runs predate maps). Raises
    FileNotFoundError when neither exists."""
    path = map_path(store, run_id)
    if path.is_file():
        return path.read_text(encoding="utf-8")
    state = store.load(run_id)
    return render_map(state)
