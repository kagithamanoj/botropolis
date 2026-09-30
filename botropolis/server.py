"""HTTP API for Botropolis.

Run with:
    uvicorn botropolis.server:app --reload

Endpoints:
    POST /ask              Route a request through the CEO orchestrator
    GET  /agents           List every registered agent
    GET  /agents/{name}    Show one agent's details
    POST /agents/{name}/ask Chat with one agent directly (war room)
    GET  /analytics        Per-agent usage totals (calls, latency, tokens)
    GET  /departments      List departments and their headcounts
    GET  /health           Liveness check
    GET  /                 Web UI (static files under /static)

The web UI lives in botropolis/web/ and needs no build step.
"""

from __future__ import annotations

from pathlib import Path

from fastapi import FastAPI, HTTPException
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field

from botropolis.core import usage
from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import AgentResult

WEB_DIR = Path(__file__).parent / "web"

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


@app.get("/", include_in_schema=False)
def index() -> FileResponse:
    """Serve the web UI."""
    return FileResponse(WEB_DIR / "index.html")


# Static assets for the web UI. Mounted after the API routes so /ask,
# /agents, and friends keep working.
app.mount("/static", StaticFiles(directory=WEB_DIR), name="static")


@app.post("/ask")
def ask(body: AskRequest) -> dict:
    """Send a request to the CEO; get back a full CompanyReport."""
    report = ceo.handle(body.request)
    for result in report.results:
        _record_usage(result)
    return report.to_dict()


def _record_usage(result: AgentResult) -> None:
    """Write one row to the usage log. Never raises."""
    meta = result.metadata or {}
    usage.log_invocation(
        agent_name=result.agent_name,
        department=result.department,
        model=str(meta.get("model", "")),
        provider=str(meta.get("provider", "stub")),
        latency_ms=result.elapsed_seconds * 1000.0,
        tokens_in=int(meta.get("tokens_in", 0) or 0),
        tokens_out=int(meta.get("tokens_out", 0) or 0),
        success=result.success,
    )


@app.post("/agents/{name}/ask")
def ask_agent(name: str, body: AskRequest) -> dict:
    """Chat with one agent directly, skipping the CEO (war room)."""
    try:
        agent = registry.get(name)
    except KeyError:
        raise HTTPException(status_code=404, detail=f"Unknown agent: {name}")
    result = agent.run(body.request)
    _record_usage(result)
    return result.to_dict()


@app.get("/analytics")
def analytics() -> dict:
    """Per-agent usage totals: calls, latency, and tokens used."""
    return usage.get_summary()


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
