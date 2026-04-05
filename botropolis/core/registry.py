"""Load agent specs from YAML and hand out Agent objects."""
import yaml
from pathlib import Path
from .agent import Agent

AGENT_DIR = Path(__file__).resolve().parent.parent / "agents"


class Registry:
    def __init__(self):
        self._agents = {}
        for spec_path in AGENT_DIR.rglob("*.yaml"):
            spec = yaml.safe_load(spec_path.read_text())
            agent = Agent(spec)
            self._agents[agent.name.lower()] = agent

    def get(self, name: str) -> Agent:
        return self._agents[name.lower()]
