"""Shared data schemas for Botropolis.

These dataclasses are the contracts passed between the CEO orchestrator,
the department agents, and the API server.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Dict, List, Optional
import time
import uuid


@dataclass
class Context:
    """Everything an agent may know beyond the task description.

    Typed and inspectable: the keys are declared here, not scattered
    across format strings in the orchestrator. Serializes to a plain
    dict (empty fields omitted) for prompts, the API, and logs.

    chat_history: recent conversation turns, packed as "role: content"
    lines, so follow-up questions resolve.
    teammates_so_far: earlier teammates' outputs in team mode, so
    later turns can build on, critique, or revise.
    extra: escape hatch for future context kinds. Prefer a named field.
    """

    chat_history: str = ""
    teammates_so_far: str = ""
    extra: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Non-empty fields as a plain dict."""
        data: Dict[str, Any] = {}
        if self.chat_history:
            data["chat_history"] = self.chat_history
        if self.teammates_so_far:
            data["teammates_so_far"] = self.teammates_so_far
        data.update(self.extra)
        return data

    def __bool__(self) -> bool:
        """A context with nothing in it counts as absent."""
        return bool(self.to_dict())


@dataclass
class ContractCheck:
    """One machine check that verifies a contracted run is done.

    A shell command run inside the agent sandbox (same denylist and
    workspace as the shell tool) plus the exit code that counts as
    passing. The agent claims the work is done; the check is the
    harness verifying the claim.
    """

    command: str
    expected_exit_code: int = 0
    name: str = ""

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


@dataclass
class CheckResult:
    """The outcome of one ContractCheck, attached to a report as
    evidence: what ran, what it returned, and the tail of its output."""

    command: str
    expected_exit_code: int
    exit_code: Optional[int]
    passed: bool
    output_tail: str = ""

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


@dataclass
class Contract:
    """The agreement for one run: what is being done, what is in and
    out of scope, and what counts as done.

    Written by whoever asks for the work, read by every agent on the
    run. The CEO attaches it to each task, agents see it in their
    prompt stated plainly, and the final report carries it back so a
    reader can check the work against what was actually agreed.

    done_when is the human-readable definition of done. checks are
    the machine-readable one: shell commands the harness runs before
    the report is finalized (see botropolis/core/evidence.py).
    """

    goal: str
    scope: List[str] = field(default_factory=list)
    exclusions: List[str] = field(default_factory=list)
    done_when: List[str] = field(default_factory=list)
    checks: List[ContractCheck] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)

    @classmethod
    def from_dict(cls, data: Optional[Dict[str, Any]]) -> Optional["Contract"]:
        """Rebuild a Contract from plain data. None in, None out."""
        if not data:
            return None
        checks = []
        for item in data.get("checks", []):
            if isinstance(item, ContractCheck):
                checks.append(item)
            elif isinstance(item, dict):
                checks.append(
                    ContractCheck(
                        command=str(item.get("command", "")),
                        expected_exit_code=int(item.get("expected_exit_code", 0)),
                        name=str(item.get("name", "")),
                    )
                )
        return cls(
            goal=str(data.get("goal", "")),
            scope=[str(item) for item in data.get("scope", [])],
            exclusions=[str(item) for item in data.get("exclusions", [])],
            done_when=[str(item) for item in data.get("done_when", [])],
            checks=checks,
        )


@dataclass
class Task:
    """A unit of work assigned to one agent.

    needs_approval names tools that may not run on this task unless
    approved is True: the approval wall. Flagged calls are blocked at
    dispatch, recorded, and reported back to the model as blocked.
    Empty list (the default) means no wall.
    """

    description: str
    department: Optional[str] = None
    agent_name: Optional[str] = None
    context: Context = field(default_factory=Context)
    contract: Optional[Contract] = None
    needs_approval: List[str] = field(default_factory=list)
    approved: bool = False
    id: str = field(default_factory=lambda: uuid.uuid4().hex[:8])

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        data = asdict(self)
        data["context"] = self.context.to_dict()
        return data


@dataclass
class ToolCall:
    """One tool invocation made by an agent during its run."""

    tool: str
    args: Dict[str, Any] = field(default_factory=dict)
    success: bool = True
    result_preview: str = ""
    elapsed_ms: float = 0.0

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


@dataclass
class AgentResult:
    """The outcome of one agent running one task.

    output is the human-readable answer (summary plus details).
    structured holds the same answer as validated data against the
    agent's declared output schema; empty when the provider is the
    offline stub, which skips validation.
    """

    agent_name: str
    department: str
    task_id: str
    output: str
    success: bool = True
    confidence: float = 0.5
    error: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)
    elapsed_seconds: float = 0.0
    tool_calls: List[ToolCall] = field(default_factory=list)
    structured: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


@dataclass
class CompanyReport:
    """The CEO's final report for one user request.

    status is "done" unless the contract carried machine checks and
    one of them failed, in which case it is "unverified" and evidence
    holds the check results.
    """

    request: str
    departments_involved: List[str] = field(default_factory=list)
    results: List[AgentResult] = field(default_factory=list)
    summary: str = ""
    contract: Optional[Contract] = None
    evidence: List[CheckResult] = field(default_factory=list)
    status: str = "done"
    elapsed_seconds: float = 0.0
    created_at: float = field(default_factory=time.time)

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        data = asdict(self)
        data["results"] = [r.to_dict() for r in self.results]
        return data

    def successful(self) -> bool:
        """True when every agent result succeeded."""
        return bool(self.results) and all(r.success for r in self.results)


@dataclass
class TeamRound:
    """One agent's turn inside a team session."""

    round_number: int
    agent_name: str
    department: str
    result: AgentResult

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


@dataclass
class TeamReport:
    """The CEO's report for one team collaboration session.

    status follows the same rule as CompanyReport: "done", or
    "unverified" when a contract check failed.
    """

    request: str
    agents: List[str] = field(default_factory=list)
    rounds: List[TeamRound] = field(default_factory=list)
    synthesis: str = ""
    contract: Optional[Contract] = None
    evidence: List[CheckResult] = field(default_factory=list)
    status: str = "done"
    elapsed_seconds: float = 0.0
    created_at: float = field(default_factory=time.time)

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        data = asdict(self)
        data["rounds"] = [r.to_dict() for r in self.rounds]
        return data

    def successful(self) -> bool:
        """True when every team round succeeded."""
        return bool(self.rounds) and all(r.result.success for r in self.rounds)
