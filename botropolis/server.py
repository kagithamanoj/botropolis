"""HTTP API for Botropolis.

Run with:
    uvicorn botropolis.server:app --reload

Endpoints:
    POST /ask              Route a request through the CEO orchestrator
    GET  /agents           List every registered agent
    GET  /agents/{name}    Show one agent's details
    POST /agents/{name}/ask Chat with one agent directly (war room)
    GET  /agents/{name}/ask/stream Stream one agent's tool calls (SSE)
    POST /team           Run a team of agents in collaboration rounds
    GET  /analytics        Per-agent usage totals (calls, latency, tokens)
    GET  /departments      List departments and their headcounts
    GET  /health           Liveness check
    GET  /                 Web UI (static files under /static)

The web UI lives in botropolis/web/ and needs no build step.
"""

from __future__ import annotations

import json
import queue
import threading
from pathlib import Path
from typing import Iterator, List, Tuple

from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import FileResponse, StreamingResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field

from botropolis.core import usage
from botropolis.core.orchestrator import CEO, MAX_TEAM_ROUNDS
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import AgentResult, Task

WEB_DIR = Path(__file__).parent / "web"

app = FastAPI(
    title="Botropolis",
    description="A company of bots: multi-agent departments coordinated by Manoj, the CEO.",
    version="0.1.0",
)

registry = AgentRegistry()
ceo = CEO(registry)


class AskRequest(BaseModel):
    """Body for POST /ask."""

    request: str = Field(..., min_length=1, description="What you want the company to do")


class TeamRequest(BaseModel):
    """Body for POST /team."""

    request: str = Field(..., min_length=1, description="The task for the team")
    agents: List[str] = Field(..., min_length=1, description="Agent names, in run order")
    rounds: int = Field(
        default=2,
        ge=1,
        le=MAX_TEAM_ROUNDS,
        description="Collaboration rounds; each agent runs once per round",
    )


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
    """Send a request to Manoj (the CEO); get back a full CompanyReport."""
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
    """Chat with one agent directly, skipping Manoj (war room)."""
    try:
        agent = registry.get(name)
    except KeyError:
        raise HTTPException(status_code=404, detail=f"Unknown agent: {name}")
    result = agent.run(body.request)
    _record_usage(result)
    return result.to_dict()


def _agent_event_stream(agent, request_text: str) -> Iterator[str]:
    """Yield server-sent events while one agent works.

    Events: tool_started, tool_finished, result, failed, done.
    The agent runs in a worker thread; its loop pushes events onto a
    queue and this generator drains it. Usage is logged in the worker
    once the run finishes.
    """
    events: "queue.Queue[Tuple[str | None, dict]]" = queue.Queue()

    def on_event(kind: str, payload: dict) -> None:
        events.put((kind, payload))

    def runner() -> None:
        try:
            task = Task(
                description=request_text,
                department=agent.department,
                agent_name=agent.name,
            )
            result = agent.run(task, on_event=on_event)
            _record_usage(result)
            events.put(("result", result.to_dict()))
        except Exception as exc:  # never leave the stream hanging
            events.put(("failed", {"message": str(exc)}))
        finally:
            events.put((None, {}))

    thread = threading.Thread(target=runner, daemon=True)
    thread.start()
    while True:
        kind, payload = events.get()
        if kind is None:
            break
        yield f"event: {kind}\ndata: {json.dumps(payload, default=str)}\n\n"
    yield "event: done\ndata: {}\n\n"


@app.get("/agents/{name}/ask/stream")
def ask_agent_stream(
    name: str, request: str = Query(..., min_length=1)
) -> StreamingResponse:
    """Chat with one agent and stream its tool calls as server-sent events.

    The war room UI uses this to show tool calls live while the agent
    works. Event kinds: tool_started, tool_finished, result, failed, done.
    """
    try:
        agent = registry.get(name)
    except KeyError:
        raise HTTPException(status_code=404, detail=f"Unknown agent: {name}")
    return StreamingResponse(
        _agent_event_stream(agent, request),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache"},
    )


@app.post("/team")
def team(body: TeamRequest) -> dict:
    """Run a team of agents in collaboration rounds.

    Each agent sees the request plus earlier teammates' outputs, so the
    team can draft, critique, and revise. Returns a TeamReport.
    """
    try:
        report = ceo.team(body.request, body.agents, rounds=body.rounds)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail=str(exc))
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc))
    for team_round in report.rounds:
        _record_usage(team_round.result)
    return report.to_dict()


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
