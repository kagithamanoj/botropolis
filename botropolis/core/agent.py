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

from botropolis.core.loop import DEFAULT_MAX_STEPS, parse_model_reply, run_tool_loop
from botropolis.core.models import ModelClient
from botropolis.core.output_schemas import (
    AgentOutput,
    OutputValidationError,
    format_instruction,
    parse_structured_output,
    resolve_output_schema,
    validated_dump,
)
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
        # toolkit is the single source of truth for what the agent may
        # actually call. There is no separate display-only tools list.
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
        # Output contract: the shape this agent's answer must take.
        # Explicit `output_schema` in the spec wins; otherwise the
        # department default applies. Unknown names fail here, at load
        # time, not mid-run.
        self.output_schema_name, self.output_schema = resolve_output_schema(
            spec.get("output_schema"), self.department
        )

    def build_messages(self, task: Task) -> List[Dict[str, str]]:
        """Compose the system and user messages for a task.

        The user message ends with the output contract: the model must
        answer as JSON matching the agent's declared schema, so the
        answer is machine-checkable downstream.
        """
        user_content = f"Task: {task.description}"
        if task.context:
            context_lines = "\n".join(f"- {k}: {v}" for k, v in task.context.items())
            user_content += f"\n\nContext:\n{context_lines}"
        user_content += f"\n\nOutput format:\n{format_instruction(self.output_schema)}"
        return [
            {"role": "system", "content": self.system_prompt},
            {"role": "user", "content": user_content},
        ]

    def _contract_output(
        self, raw_text: str, base_messages: List[Dict[str, str]]
    ) -> tuple[str, Dict[str, Any]]:
        """Validate raw model text against the agent's output schema.

        Returns (human-readable output, structured dict). The offline
        stub skips validation: its output is labeled offline prose, not
        a model answer, so it is wrapped as the summary verbatim.

        On validation failure the model gets exactly one repair attempt
        with the errors spelled out. If the repair also fails, raises
        OutputValidationError, which run() turns into a failed
        AgentResult. A failed contract is reported, never silently
        passed through as good output.
        """
        provider = self.client.provider_for(self.model)
        if provider == "stub":
            text = raw_text.strip()
            wrapped = self.output_schema(summary=text or "(no output)")
            return text, validated_dump(wrapped)
        try:
            model = parse_structured_output(raw_text, self.output_schema)
        except OutputValidationError as first:
            repair_messages = base_messages + [
                {"role": "assistant", "content": raw_text},
                {
                    "role": "user",
                    "content": (
                        "Your last reply did not match the required output schema:\n"
                        f"{first}\n"
                        "Reply with ONLY a corrected JSON object matching the schema."
                    ),
                },
            ]
            repaired = self.client.chat(self.model, repair_messages)
            kind, parsed = parse_model_reply(repaired)
            if kind == "final":
                repaired = parsed
            model = parse_structured_output(repaired, self.output_schema)
        return self._render_output(model), validated_dump(model)

    @staticmethod
    def _render_output(model: AgentOutput) -> str:
        """Render a validated answer as human-readable prose.

        Summary first, details after. This keeps `output` readable for
        the CEO, team context, and the API while `structured` carries
        the machine-checked contract.
        """
        if model.details.strip():
            return f"{model.summary.strip()}\n\n{model.details.strip()}"
        return model.summary.strip()

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
            messages = self.build_messages(task)
            raw = self.client.chat(self.model, messages)
            # Same answer semantics as the tool loop: a FINAL: prefix marks
            # the answer and is stripped, anything else is the answer as-is.
            kind, parsed = parse_model_reply(raw)
            if kind == "final":
                raw = parsed
            output, structured = self._contract_output(raw, messages)
            provider = self.client.provider_for(self.model)
            usage = getattr(self.client, "last_usage", None) or {}
            return AgentResult(
                agent_name=self.name,
                department=self.department,
                task_id=task.id,
                output=output,
                structured=structured,
                success=True,
                confidence=0.85 if provider != "stub" else 0.5,
                metadata={
                    "model": self.model,
                    "provider": provider,
                    "stub": provider == "stub",
                    "tokens_in": int(usage.get("input", 0)),
                    "tokens_out": int(usage.get("output", 0)),
                    "output_schema": self.output_schema_name,
                    "output_validated": provider != "stub",
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
        """Run the think-act-observe loop and wrap it in an AgentResult.

        The loop's final answer goes through the same output contract
        as the single-shot path: validated against the schema, one
        repair attempt on failure.
        """
        raw_output, tool_calls, usage = run_tool_loop(self, task, on_event=on_event)
        output, structured = self._contract_output(raw_output, self.build_messages(task))
        provider = self.client.provider_for(self.model)
        return AgentResult(
            agent_name=self.name,
            department=self.department,
            task_id=task.id,
            output=output,
            structured=structured,
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
                "output_schema": self.output_schema_name,
                "output_validated": provider != "stub",
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
            "tools": self.toolkit,
            "toolkit": self.toolkit,
            "output_schema": self.output_schema_name,
            "example_tasks": self.example_tasks,
        }
