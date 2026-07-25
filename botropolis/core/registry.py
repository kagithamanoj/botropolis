"""AgentRegistry: the company directory of Botropolis.

Discovers every agent spec under botropolis/agents/<department>/*.yaml,
instantiates an Agent per spec, and answers lookups by name or department.
"""

from __future__ import annotations

from pathlib import Path
from typing import Dict, List, Optional

from botropolis.core.agent import Agent
from botropolis.core.models import ModelClient


def default_agents_dir() -> Path:
    """Return the bundled agents directory."""
    return Path(__file__).resolve().parents[1] / "agents"


class AgentRegistry:
    """Discovers and serves all registered agents."""

    def __init__(
        self,
        agents_dir: Optional[Path] = None,
        client: Optional[ModelClient] = None,
    ) -> None:
        self.agents_dir = Path(agents_dir) if agents_dir else default_agents_dir()
        self.client = client or ModelClient()
        self._agents: Dict[str, Agent] = {}
        self.discover()

    def discover(self) -> int:
        """Scan for spec files and register every agent found.

        Returns the number of agents registered.
        """
        self._agents = {}
        for spec_path in sorted(self.agents_dir.glob("*/*.yaml")):
            if spec_path.name.startswith("_"):
                continue
            agent = Agent(spec_path, client=self.client)
            key = agent.name.lower()
            if key in self._agents:
                raise ValueError(f"Duplicate agent name: {agent.name}")
            self._agents[key] = agent
        return len(self._agents)

    def get(self, name: str) -> Agent:
        """Return the agent with this name (case-insensitive)."""
        agent = self._agents.get(name.lower())
        if agent is None:
            raise KeyError(f"Unknown agent: {name}")
        return agent

    def by_department(self, department: str) -> List[Agent]:
        """Return all agents in a department, sorted by name."""
        dept = department.lower()
        return sorted(
            (a for a in self._agents.values() if a.department.lower() == dept),
            key=lambda a: a.name,
        )

    def list_all(self) -> List[Agent]:
        """Return every agent, sorted by department then name."""
        return sorted(
            self._agents.values(), key=lambda a: (a.department.lower(), a.name.lower())
        )

    @property
    def departments(self) -> List[str]:
        """Return the sorted list of department names."""
        return sorted({a.department for a in self._agents.values()})

    def __len__(self) -> int:
        return len(self._agents)

    def __contains__(self, name: str) -> bool:
        return name.lower() in self._agents
