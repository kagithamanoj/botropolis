"""HTTP API for Botropolis.

Run with:
    uvicorn botropolis.server:app --reload

Endpoints:
    POST /ask           Route a request through the CEO orchestrator
    GET  /agents        List every registered agent
    GET  /agents/{name} Show one agent's details
    GET  /departments   List departments and their headcounts
    GET  /health        Liveness check
"""

from __future__ import annotations

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry

app = FastAPI(
    title="Botropolis",
    description="A company of bots: multi-agent departments coordinated by a CEO orchestrator.",
    version="0.1.0",
)

registry = AgentRegistry()
ceo = CEO(registry)


class AskRequest(BaseModel):
    """Body for POST /ask."""

    request: str = Field(..., min_length=1, description="What you want the company to do")


@app.get("/health")
def health() -> dict:
    """Liveness check."""
    return {"status": "ok", "agents": len(registry)}


@app.post("/ask")
def ask(body: AskRequest) -> dict:
    """Send a request to the CEO; get back a full CompanyReport."""
    report = ceo.handle(body.request)
    return report.to_dict()


@app.get("/agents")
def list_agents() -> dict:
    """List every registered agent."""
    return {"agents": [a.describe() for a in registry.list_all()]}


@app.get("/agents/{name}")
def get_agent(name: str) -> dict:
    """Show one agent's details by name."""
    try:
        return registry.get(name).describe()
    except KeyError:
        raise HTTPException(status_code=404, detail=f"Unknown agent: {name}")


@app.get("/departments")
def list_departments() -> dict:
    """List departments with agent counts."""
    return {
        "departments": [
            {"name": dept, "agents": [a.name for a in registry.by_department(dept)]}
            for dept in registry.departments
        ]
    }
