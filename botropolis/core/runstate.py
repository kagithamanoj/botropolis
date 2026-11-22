"""Durable run state for tool-using agent runs.

The ledger records what happened; run state is what a run needs to
continue. While a tool loop runs, it writes its progress to a JSON
file after every step: the task, the contract, and each completed
tool call with its result. If the process dies mid-run, the state on
disk is where the run stopped, and `botropolis resume <run_id>`
rebuilds the conversation from the completed steps and carries on
from the first unfinished one instead of starting over.

State files live in botropolis/data/runs/ (gitignored), next to the
usage log. BOTROPOLIS_RUNS_DIR moves the directory, which is how
tests keep runs hermetic.

State is a side channel like the ledger: a failed write is swallowed,
never a reason to break the run itself.
"""

from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import Any, Dict, Optional

from botropolis.core.schemas import Context, Contract, Task


def runs_dir() -> Path:
    """Return the run state directory, creating it on first use."""
    override = os.environ.get("BOTROPOLIS_RUNS_DIR")
    if override:
        root = Path(override).expanduser().resolve()
    else:
        root = Path(__file__).resolve().parents[1] / "data" / "runs"
    root.mkdir(parents=True, exist_ok=True)
    return root


class RunStore:
    """Reads and writes per-run state files."""

    def __init__(self, root: Optional[Path] = None) -> None:
        self.root = Path(root) if root is not None else runs_dir()

    def path_for(self, run_id: str) -> Path:
        return self.root / f"{run_id}.json"

    def save(self, state: Dict[str, Any]) -> None:
        """Write one run's state, replacing any earlier snapshot."""
        self.root.mkdir(parents=True, exist_ok=True)
        state = dict(state)
        state["updated_at"] = time.time()
        path = self.path_for(str(state["run_id"]))
        path.write_text(json.dumps(state, indent=2, default=str), encoding="utf-8")

    def load(self, run_id: str) -> Dict[str, Any]:
        """Read one run's state. Raises FileNotFoundError when absent."""
        path = self.path_for(run_id)
        if not path.is_file():
            raise FileNotFoundError(f"No saved run with id {run_id}")
        return json.loads(path.read_text(encoding="utf-8"))

    def exists(self, run_id: str) -> bool:
        return self.path_for(run_id).is_file()


def task_from_state(state: Dict[str, Any]) -> Task:
    """Rebuild the Task a saved run was working on."""
    data = state.get("task") or {}
    context_data = data.get("context") or {}
    extra = {
        key: value
        for key, value in context_data.items()
        if key not in ("chat_history", "teammates_so_far")
    }
    context = Context(
        chat_history=str(context_data.get("chat_history", "")),
        teammates_so_far=str(context_data.get("teammates_so_far", "")),
        extra=extra,
    )
    contract = Contract.from_dict(state.get("contract") or data.get("contract"))
    return Task(
        description=str(data.get("description", "")),
        department=data.get("department"),
        agent_name=data.get("agent_name"),
        context=context,
        contract=contract,
        id=str(data.get("id") or "resumed"),
    )
