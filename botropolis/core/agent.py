"""Base agent class."""
from dataclasses import dataclass, field


@dataclass
class Task:
    description: str
    department: str = ""
    agent_name: str = ""


@dataclass
class Result:
    output: str
    agent_name: str = ""
    tokens_used: int = 0


class Agent:
    """Base class every specialist agent extends."""

    def __init__(self, spec: dict):
        self.name: str = spec["name"]
        self.department: str = spec.get("department", "")
        self.system_prompt: str = spec.get("system_prompt", "")

    def run(self, task: Task) -> Result:
        raise NotImplementedError
