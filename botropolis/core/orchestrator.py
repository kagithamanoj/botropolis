"""The CEO orchestrator of Botropolis.

The CEO takes a user request, decomposes it into subtasks, routes each
subtask to the right department using keyword matching against the agent
registry, collects the agents' results, and synthesizes a CompanyReport.
"""

from __future__ import annotations

import time
from typing import Dict, List, Optional, Tuple

from botropolis.core.agent import Agent
from botropolis.core.models import ModelClient
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import AgentResult, CompanyReport, Task, TeamReport, TeamRound

# Keywords that suggest a department should be involved. Ordered by
# department so routing is deterministic.
DEPARTMENT_KEYWORDS: Dict[str, Tuple[str, ...]] = {
    "research": (
        "research", "find", "search", "look up", "what is", "what are", "explain",
        "report", "news", "trend", "fact", "compare", "survey", "overview",
    ),
    "health": (
        "health", "symptom", "sleep", "fitness", "diet", "exercise", "wellness",
        "medical", "doctor", "workout", "nutrition",
    ),
    "finance": (
        "finance", "stock", "market", "invest", "budget", "money", "equity",
        "portfolio", "price", "valuation", "spending",
    ),
    "code": (
        "code", "python", "function", "script", "bug", "debug", "program",
        "api", "deploy", "ci/cd", "pipeline", "refactor", "review the code",
    ),
    "data": (
        "data", "dataset", "pipeline", "etl", "model training", "train a model",
        "machine learning", "ml ", "feature",
    ),
    "legal": (
        "legal", "law", "contract", "compliance", "regulation", "clause",
        "liability", "terms",
    ),
    "marketing": (
        "marketing", "copy", "seo", "blog", "headline", "brand", "campaign",
        "landing page",
    ),
    "ops": (
        "schedule", "calendar", "meeting", "email", "inbox", "reminder",
        "organize", "plan my",
    ),
    "security": (
        "security", "vulnerability", "vuln", "secure", "breach", "pentest",
        "audit", "cve", "threat",
    ),
    "support": (
        "support", "help me with", "customer", "ticket", "complaint",
        "troubleshoot",
    ),
}

# Preferred agent per department for whole-request routing.
LEAD_AGENT: Dict[str, str] = {
    "research": "Aarav",
    "health": "Nina",
    "finance": "Priya",
    "code": "Arjun",
    "data": "Vikram",
    "legal": "Raj",
    "marketing": "Zara",
    "ops": "Kofi",
    "security": "Ishaan",
    "support": "Tara",
}

# Hard ceiling on team collaboration rounds. Each round runs every agent,
# so costs grow with agents * rounds; three is enough for draft, critique,
# revise without runaway bills.
MAX_TEAM_ROUNDS = 3

# How much of each teammate's prior output to include in the next prompt.
# Enough for continuity, not so much that prompts explode.
PRIOR_OUTPUT_CHARS = 800


class CEO:
    """Orchestrates the company: plans work, assigns agents, reports back."""

    # The CEO has a name. Agents are addressed by first name everywhere.
    name = "Manoj"

    def __init__(
        self,
        registry: Optional[AgentRegistry] = None,
        client: Optional[ModelClient] = None,
    ) -> None:
        self.registry = registry or AgentRegistry()
        self.client = client or ModelClient()

    def score_departments(self, request: str) -> List[Tuple[str, int]]:
        """Score departments by keyword hits, highest first."""
        lowered = request.lower()
        scores = []
        for dept, keywords in DEPARTMENT_KEYWORDS.items():
            if dept not in self.registry.departments:
                continue
            hits = sum(1 for kw in keywords if kw in lowered)
            if hits:
                scores.append((dept, hits))
        return sorted(scores, key=lambda item: (-item[1], item[0]))

    def plan(self, request: str) -> List[Task]:
        """Decompose a request into one task per relevant department."""
        scored = self.score_departments(request)
        departments = [dept for dept, _ in scored[:3]]
        if not departments:
            departments = ["research"] if "research" in self.registry.departments else [
                self.registry.departments[0]
            ]
        tasks = []
        for dept in departments:
            focus = f" (focus: {request[:80]})" if len(departments) > 1 else ""
            tasks.append(
                Task(
                    description=f"{request}{focus}",
                    department=dept,
                )
            )
        return tasks

    def assign(self, task: Task) -> Agent:
        """Pick the best agent in the task's department."""
        agents = self.registry.by_department(task.department or "")
        if not agents:
            raise KeyError(f"No agents in department: {task.department}")
        if task.agent_name:
            return self.registry.get(task.agent_name)
        lead = LEAD_AGENT.get((task.department or "").lower())
        if lead:
            try:
                return self.registry.get(lead)
            except KeyError:
                pass
        return agents[0]

    def handle(self, request: str) -> CompanyReport:
        """Run the full company workflow for one user request."""
        started = time.time()
        tasks = self.plan(request)
        results: List[AgentResult] = []
        departments: List[str] = []
        for task in tasks:
            agent = self.assign(task)
            task.agent_name = agent.name
            result = agent.run(task)
            results.append(result)
            if agent.department not in departments:
                departments.append(agent.department)
        summary = self._synthesize(request, results)
        return CompanyReport(
            request=request,
            departments_involved=departments,
            results=results,
            summary=summary,
            elapsed_seconds=time.time() - started,
        )

    def team(self, request: str, agent_names: List[str], rounds: int = 2) -> TeamReport:
        """Run named agents as a team in collaboration rounds.

        Each agent's prompt includes the original request plus the outputs
        of every teammate that ran before it, so later turns can build on,
        critique, or revise earlier work. Typical shape: Arjun drafts,
        Sofia critiques, Arjun revises.

        Rounds are capped at MAX_TEAM_ROUNDS. Unknown agent names raise
        KeyError; an empty team raises ValueError.
        """
        if not agent_names:
            raise ValueError("team() needs at least one agent")
        agents: List[Agent] = []
        for name in agent_names:
            try:
                agents.append(self.registry.get(name))
            except KeyError:
                raise KeyError(f"Unknown agent: {name}")
        rounds = max(1, min(int(rounds), MAX_TEAM_ROUNDS))

        started = time.time()
        team_rounds: List[TeamRound] = []
        prior: List[Tuple[int, str, str]] = []  # (round_number, agent_name, output)
        for round_num in range(1, rounds + 1):
            for agent in agents:
                task = Task(
                    description=f"Team request (round {round_num} of {rounds}): {request}",
                    department=agent.department,
                    agent_name=agent.name,
                    context=self._team_context(prior),
                )
                result = agent.run(task)
                team_rounds.append(
                    TeamRound(
                        round_number=round_num,
                        agent_name=agent.name,
                        department=agent.department,
                        result=result,
                    )
                )
                prior.append((round_num, agent.name, result.output))

        return TeamReport(
            request=request,
            agents=[a.name for a in agents],
            rounds=team_rounds,
            synthesis=self._synthesize_team(request, agents, team_rounds),
            elapsed_seconds=time.time() - started,
        )

    def _team_context(self, prior: List[Tuple[int, str, str]]) -> Dict[str, str]:
        """Pack earlier teammates' outputs into task context."""
        if not prior:
            return {}
        chunks = []
        for round_num, agent_name, output in prior:
            snippet = output.strip()[:PRIOR_OUTPUT_CHARS]
            chunks.append(f"[round {round_num} - {agent_name}]\n{snippet}")
        return {"teammates_so_far": "\n\n".join(chunks)}

    def _synthesize_team(
        self, request: str, agents: List[Agent], team_rounds: List[TeamRound]
    ) -> str:
        """Summarize the final state of a team session."""
        lines = [
            f"Team request: {request}",
            f"Team: {', '.join(a.name for a in agents)} "
            f"({len(team_rounds)} runs over "
            f"{max(r.round_number for r in team_rounds)} rounds)",
            "",
            "Final state per agent:",
        ]
        final: Dict[str, TeamRound] = {}
        for team_round in team_rounds:
            final[team_round.agent_name] = team_round
        for name, team_round in final.items():
            status = "done" if team_round.result.success else "failed"
            output = team_round.result.output.strip()
            first_line = output.splitlines()[0] if output else "(no output)"
            lines.append(f"[{name}] {status}: {first_line}")
        if team_rounds and all(r.result.metadata.get("stub") for r in team_rounds):
            lines.append("")
            lines.append(
                "All runs used the offline stub: reasoning shape only, "
                "no live model calls."
            )
        return "\n".join(lines)

    def _synthesize(self, request: str, results: List[AgentResult]) -> str:
        """Combine agent outputs into an executive summary."""
        lines = [
            f"Request: {request}",
            f"Departments consulted: {', '.join(r.department for r in results)}",
            "",
        ]
        for result in results:
            status = "done" if result.success else "failed"
            first_line = result.output.strip().splitlines()[0] if result.output.strip() else "(no output)"
            lines.append(f"[{result.department} / {result.agent_name}] {status}: {first_line}")
        lines.append("")
        lines.append(
            "Full per-agent outputs are included in the report results. "
            "Consult a human specialist before acting on health, legal, or financial guidance."
        )
        return "\n".join(lines)
