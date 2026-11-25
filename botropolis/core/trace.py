"""Trace export: read a run back as a trace.

The ledger and run state already record every step, but there was
no way to read a run back as one story. build_trace() assembles it:
the task, the plan, the tool calls with one-line outcomes, the
ledger events for the run, and the final status and answer. The
CLI prints it as plain readable text, or as JSON with --json.

The run state is the source of truth: a trace needs it, and a
missing run is an error. Ledger events are included when a
persisted ledger exists (BOTROPOLIS_LEDGER_DB, or
botropolis/data/ledger.db when present); without one, the trace
carries an empty events list instead of failing. Reading a trace
never creates files: a ledger path that does not exist is simply
skipped.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Dict, List, Optional

from botropolis.core.runstate import RunStore
from botropolis.core.taskmap import plan_steps

DEFAULT_LEDGER_NAME = "ledger.db"


def ledger_db_path() -> Optional[Path]:
    """Where a persisted ledger would live, if one exists.

    BOTROPOLIS_LEDGER_DB wins; otherwise the default file under
    botropolis/data. None when no ledger file exists, so callers
    never create one by accident.
    """
    override = os.environ.get("BOTROPOLIS_LEDGER_DB")
    if override:
        path = Path(override).expanduser().resolve()
        return path if path.is_file() else None
    default = Path(__file__).resolve().parents[1] / "data" / DEFAULT_LEDGER_NAME
    return default if default.is_file() else None


def _ledger_events(run_id: str, ledger_path: Optional[Path]) -> List[Dict[str, Any]]:
    """Ledger events for one run, in order. Empty when there is no
    ledger to read. Never raises."""
    if ledger_path is None:
        return []
    try:
        from botropolis.core.ledger import Ledger

        ledger = Ledger(ledger_path)
        try:
            return ledger.events(run_id)
        finally:
            ledger.close()
    except Exception:
        return []


def build_trace(
    store: RunStore,
    run_id: str,
    ledger_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """Assemble one run's trace. Raises FileNotFoundError when the
    run does not exist. ledger_path, when given, points at a
    persisted ledger DB; otherwise the default is looked up."""
    state = store.load(run_id)
    if ledger_path is None:
        ledger_path = ledger_db_path()
    task = state.get("task") or {}
    steps = [
        {
            "tool": str(step.get("tool", "")),
            "args": dict(step.get("args") or {}),
            "success": bool(step.get("success", True)),
            "outcome": str(step.get("result_preview", "")),
        }
        for step in (state.get("steps") or [])
    ]
    return {
        "run_id": str(state.get("run_id", run_id)),
        "agent": str(state.get("agent", "")),
        "task": str(task.get("description", "")),
        "status": str(state.get("status", "")),
        "reason": str(state.get("reason") or ""),
        "plan": plan_steps(state.get("plan")),
        "steps": steps,
        "events": _ledger_events(run_id, ledger_path),
        "answer": str(state.get("answer") or ""),
    }


def _one_line(text: str, limit: int = 120) -> str:
    flat = " ".join(str(text or "").split())
    if len(flat) > limit:
        return flat[:limit] + "..."
    return flat or "(empty result)"


def render_text(trace: Dict[str, Any]) -> str:
    """Render a trace dict as plain readable text."""
    lines = [f"Trace: {trace.get('run_id', '')}", ""]
    lines.append(f"Agent: {trace.get('agent', '')}")
    lines.append(f"Task: {trace.get('task', '')}")
    lines.append(f"Status: {trace.get('status', '')}")
    if trace.get("reason"):
        lines.append(f"Reason: {trace['reason']}")
    lines.append("")
    plan = trace.get("plan") or []
    if plan:
        lines.append("Plan:")
        for index, step in enumerate(plan, start=1):
            lines.append(f"  {index}. {step}")
        lines.append("")
    steps = trace.get("steps") or []
    lines.append("Steps:")
    if not steps:
        lines.append("  none yet")
    for index, step in enumerate(steps, start=1):
        outcome = "ok" if step.get("success", True) else "FAILED"
        lines.append(
            f"  {index}. {step.get('tool', '')} "
            f"{_one_line(step.get('args', {}))} "
            f"-> {outcome}: {_one_line(step.get('outcome', ''))}"
        )
    lines.append("")
    events = trace.get("events") or []
    if events:
        lines.append("Events:")
        for event in events:
            payload = json.dumps(event.get("payload", {}), default=str)
            lines.append(
                f"  {event.get('seq', '')}. "
                f"[{event.get('actor', '')}] {event.get('kind', '')}: "
                f"{_one_line(payload)}"
            )
        lines.append("")
    if trace.get("answer"):
        lines.append("Answer:")
        lines.append(str(trace["answer"]).strip())
        lines.append("")
    return "\n".join(lines).strip() + "\n"
