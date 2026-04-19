"""CEO orchestrator: routes a request to the right department's lead agent."""
from .agent import Task, Result
from .registry import Registry

LEAD_AGENT = {
    "research": "Scout",
}

KEYWORDS = {
    "research": ("research", "find", "search", "look up"),
}


class Orchestrator:
    """The CEO. Takes a request, picks a department, runs its lead agent."""

    def __init__(self, registry: Registry):
        self._registry = registry

    def ask(self, request: str) -> Result:
        dept = "research"
        for d, words in KEYWORDS.items():
            if any(w in request.lower() for w in words):
                dept = d
                break
        agent = self._registry.get(LEAD_AGENT[dept])
        return agent.run(Task(description=request, department=dept,
                              agent_name=agent.name))
