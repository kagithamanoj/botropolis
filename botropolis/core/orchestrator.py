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
from botropolis.core.schemas import AgentResult, CompanyReport, Task

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
    "research": "scout",
    "health": "TriageBot",
    "finance": "MarketAnalyst",
    "code": "coder",
    "data": "DataEngineer",
    "legal": "paralegal",
    "marketing": "copywriter",
    "ops": "scheduler",
    "security": "SecAuditor",
    "support": "SupportAgent",
}


class CEO:
    """Orchestrates the company: plans work, assigns agents, reports back."""

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
