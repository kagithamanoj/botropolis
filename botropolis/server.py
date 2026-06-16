"""FastAPI prototype: POST /ask routes through the CEO orchestrator."""
from fastapi import FastAPI
from pydantic import BaseModel
from .core.registry import Registry
from .core.orchestrator import Orchestrator

app = FastAPI(title="Botropolis")
_orchestrator = Orchestrator(Registry())


class AskRequest(BaseModel):
    request: str


@app.post("/ask")
def ask(req: AskRequest):
    result = _orchestrator.ask(req.request)
    return {"agent": result.agent_name, "output": result.output}
