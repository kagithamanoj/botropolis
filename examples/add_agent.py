"""How to register a new agent programmatically.

There are two supported paths:

1. Drop a spec file in (recommended for permanent agents):
   write botropolis/agents/<department>/<name>.yaml, following any existing
   spec as a template. AgentRegistry discovers it on the next run.

2. Register at runtime (shown below): write the spec dict to a temp YAML
   file and load it with Agent, or point a registry at a custom directory.

This script demonstrates path 2 by creating a "TravelPlanner" agent in a
scratch directory, registering it, and running a task through it.
"""

import sys
import tempfile
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from botropolis.core.agent import Agent
from botropolis.core.models import ModelClient
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Task

# A spec is just a dict with the required fields. Keep the system prompt
# focused: role, method, and what good output looks like.
SPEC = {
    "name": "TravelPlanner",
    "title": "Travel Planning Specialist",
    "department": "ops",
    "specialty": "Building practical trip itineraries within a budget",
    "model": "gpt-4o",
    "toolkit": ["web_search", "calculator", "current_time"],
    "system_prompt": (
        "You are TravelPlanner, the travel specialist at Botropolis. "
        "You build trip plans that respect real constraints: budget, dates, "
        "and how much moving around the traveler actually wants to do. "
        "You give a day by day sketch with costs broken out, plus one "
        "cheaper and one nicer alternative for the big choices. "
        "You flag anything that needs booking ahead."
    ),
    "example_tasks": [
        "Plan a 4 day trip to Chicago under 1200 dollars",
        "Build a packing checklist for a week in Tokyo",
    ],
}


def main() -> None:
    # Write the spec to a scratch department directory...
    with tempfile.TemporaryDirectory() as tmpdir:
        dept_dir = Path(tmpdir) / "agents" / "ops"
        dept_dir.mkdir(parents=True)
        spec_path = dept_dir / "travelplanner.yaml"
        with open(spec_path, "w", encoding="utf-8") as fh:
            yaml.safe_dump(SPEC, fh)

        # ...and load it through the normal registry path.
        registry = AgentRegistry(agents_dir=dept_dir.parent, client=ModelClient())
        agent = registry.get("TravelPlanner")
        print(f"registered: {agent.name} ({agent.title}) in {agent.department}")

        result = agent.run(Task(description="Plan a 3 day weekend in Austin under $900"))
        print(f"success={result.success} provider={result.metadata.get('provider')}")
        print(result.output[:600])


if __name__ == "__main__":
    main()
