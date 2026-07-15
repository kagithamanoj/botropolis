"""Botropolis: a company of bots.

A multi-agent framework where specialized AI agents are organized like
company departments, coordinated by a CEO orchestrator that routes tasks
to the right team.
"""

__version__ = "0.1.0"
__all__ = [
    "Agent",
    "AgentRegistry",
    "CEO",
    "ModelClient",
    "Task",
    "AgentResult",
    "CompanyReport",
]


def __getattr__(name):
    if name == "Agent":
        from botropolis.core.agent import Agent

        return Agent
    if name == "AgentRegistry":
        from botropolis.core.registry import AgentRegistry

        return AgentRegistry
    if name == "CEO":
        from botropolis.core.orchestrator import CEO

        return CEO
    if name == "ModelClient":
        from botropolis.core.models import ModelClient

        return ModelClient
    if name in ("Task", "AgentResult", "CompanyReport"):
        from botropolis.core import schemas

        return getattr(schemas, name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
