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
class Task:
    """A unit of work assigned to one agent."""

    description: str
    department: Optional[str] = None
    agent_name: Optional[str] = None
    context: Dict[str, Any] = field(default_factory=dict)
    id: str = field(default_factory=lambda: uuid.uuid4().hex[:8])

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


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
    """The outcome of one agent running one task."""

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

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


@dataclass
class CompanyReport:
    """The CEO's final report for one user request."""

    request: str
    departments_involved: List[str] = field(default_factory=list)
    results: List[AgentResult] = field(default_factory=list)
    summary: str = ""
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
    """The CEO's report for one team collaboration session."""

    request: str
    agents: List[str] = field(default_factory=list)
    rounds: List[TeamRound] = field(default_factory=list)
    synthesis: str = ""
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
