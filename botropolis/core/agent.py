"""The Agent base class: every Botropolis employee is an Agent.

An agent is defined by a YAML spec file (see botropolis/agents/*/) and is
executed through a ModelClient. When no model API key is configured, the
agent runs against the deterministic offline stub so demos and tests work
anywhere.
"""

from __future__ import annotations

import time
from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

from botropolis.core.loop import DEFAULT_MAX_STEPS, run_tool_loop
from botropolis.core.models import ModelClient
from botropolis.core.schemas import AgentResult, Task
from botropolis.tools import TOOLS

REQUIRED_SPEC_FIELDS = (
    "name",
    "title",
    "department",
    "specialty",
    "model",
    "system_prompt",
)


class Agent:
    """A specialist employee of Botropolis, loaded from a YAML spec."""

    def __init__(self, spec_path: Path, client: Optional[ModelClient] = None) -> None:
        self.spec_path = Path(spec_path)
        with open(self.spec_path, "r", encoding="utf-8") as fh:
            spec = yaml.safe_load(fh) or {}
        missing = [f for f in REQUIRED_SPEC_FIELDS if f not in spec]
        if missing:
            raise ValueError(f"{self.spec_path}: missing fields {missing}")
        self.spec: Dict[str, Any] = spec
        self.name: str = spec["name"]
        self.title: str = spec["title"]
        self.role: str = spec["title"]
        self.department: str = spec["department"]
        self.specialty: str = spec["specialty"]
        self.model: str = spec["model"]
        self.system_prompt: str = spec["system_prompt"]
        self.tools: List[str] = list(spec.get("tools", []))
        self.toolkit: List[str] = list(spec.get("toolkit", []))
        self.max_steps: int = int(spec.get("max_steps", DEFAULT_MAX_STEPS))
        unknown = [t for t in self.toolkit if t not in TOOLS]
        if unknown:
            raise ValueError(
                f"{self.spec_path}: unknown tools in toolkit: {unknown}. "
                f"Available: {sorted(TOOLS)}"
            )
        self.example_tasks: List[str] = list(spec.get("example_tasks", []))
        self.client = client or ModelClient()

    def build_messages(self, task: Task) -> List[Dict[str, str]]:
        """Compose the system and user messages for a task."""
        user_content = f"Task: {task.description}"
        if task.context:
            context_lines = "\n".join(f"- {k}: {v}" for k, v in task.context.items())
            user_content += f"\n\nContext:\n{context_lines}"
        return [
            {"role": "system", "content": self.system_prompt},
            {"role": "user", "content": user_content},
        ]

    def run(self, task: Task | str, on_event=None) -> AgentResult:
        """Run one task and return an AgentResult.

        Agents with a toolkit run the think-act-observe loop; agents
        without one do a single model call, as before. Uses the configured
        model when credentials exist, otherwise the offline stub. The stub
        response is always labeled as offline output.

        on_event is passed to the tool loop when the agent has a toolkit;
        see run_tool_loop for the event kinds.
        """
        if isinstance(task, str):
            task = Task(description=task, department=self.department, agent_name=self.name)
        started = time.time()
        try:
            if self.toolkit:
                return self._run_with_tools(task, started, on_event=on_event)
            output = self.client.chat(self.model, self.build_messages(task))
            provider = self.client.provider_for(self.model)
            usage = getattr(self.client, "last_usage", None) or {}
            return AgentResult(
                agent_name=self.name,
                department=self.department,
                task_id=task.id,
                output=output,
                success=True,
                confidence=0.85 if provider != "stub" else 0.5,
                metadata={
                    "model": self.model,
                    "provider": provider,
                    "stub": provider == "stub",
                    "tokens_in": int(usage.get("input", 0)),
                    "tokens_out": int(usage.get("output", 0)),
                },
                elapsed_seconds=time.time() - started,
            )
        except Exception as exc:  # never let one agent crash the company
            return AgentResult(
                agent_name=self.name,
                department=self.department,
                task_id=task.id,
                output="",
                success=False,
                confidence=0.0,
                error=str(exc),
                metadata={"model": self.model},
                elapsed_seconds=time.time() - started,
            )

    def _run_with_tools(self, task: Task, started: float, on_event=None) -> AgentResult:
        """Run the think-act-observe loop and wrap it in an AgentResult."""
        output, tool_calls, usage = run_tool_loop(self, task, on_event=on_event)
        provider = self.client.provider_for(self.model)
        return AgentResult(
            agent_name=self.name,
            department=self.department,
            task_id=task.id,
            output=output,
            success=True,
            confidence=0.85 if provider != "stub" else 0.5,
            metadata={
                "model": self.model,
                "provider": provider,
                "stub": provider == "stub",
                "tokens_in": int(usage.get("input", 0)),
                "tokens_out": int(usage.get("output", 0)),
                "toolkit": self.toolkit,
                "tool_calls": len(tool_calls),
            },
            elapsed_seconds=time.time() - started,
            tool_calls=tool_calls,
        )

    def describe(self) -> Dict[str, Any]:
        """Return a short public description of this agent."""
        return {
            "name": self.name,
            "title": self.title,
            "department": self.department,
            "specialty": self.specialty,
            "model": self.model,
            "tools": self.tools,
            "toolkit": self.toolkit,
            "example_tasks": self.example_tasks,
        }
