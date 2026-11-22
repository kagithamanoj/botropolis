"""Approval wall tests: flagged tools do not run on unapproved
runs, run fine when approved, and leave everything else alone."""

import yaml
from fastapi.testclient import TestClient

from botropolis.core.agent import Agent
from botropolis.core.ledger import Ledger
from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Task
from botropolis.server import app
from botropolis.tools import get_tool

client = TestClient(app)

CALC_ACTION = 'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}'
TIME_ACTION = 'ACTION: {"tool": "current_time", "args": {}}'


class FakeClient:
    """Scripted stand-in for ModelClient. Replies play in order."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


def make_agent(tmp_path, replies):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
        "toolkit": ["calculator", "current_time"],
    }
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=FakeClient(replies))


def count_executions(monkeypatch, tool_name):
    executions = []
    tool = get_tool(tool_name)
    real_execute = tool.execute

    def counting_execute(**kwargs):
        executions.append(kwargs)
        return real_execute(**kwargs)

    monkeypatch.setattr(tool, "execute", counting_execute)
    return executions


def test_flagged_tool_is_blocked_without_approval(tmp_path, monkeypatch):
    executions = count_executions(monkeypatch, "calculator")
    agent = make_agent(tmp_path, [CALC_ACTION, "FINAL: could not compute"])
    task = Task(
        description="what is 6 times 7",
        department="research",
        needs_approval=["calculator"],
        approved=False,
    )
    result = agent.run(task)
    assert executions == []
    assert len(result.tool_calls) == 1
    call = result.tool_calls[0]
    assert call.success is False
    assert "needs human approval" in call.result_preview
    # The run itself still completes: the model worked around the wall.
    assert result.success is True
    assert result.output == "could not compute"


def test_flagged_tool_runs_with_approval(tmp_path, monkeypatch):
    executions = count_executions(monkeypatch, "calculator")
    agent = make_agent(tmp_path, [CALC_ACTION, "FINAL: 42"])
    task = Task(
        description="what is 6 times 7",
        department="research",
        needs_approval=["calculator"],
        approved=True,
    )
    result = agent.run(task)
    assert len(executions) == 1
    assert result.tool_calls[0].success is True
    assert result.tool_calls[0].result_preview == "42"
    assert result.output == "42"


def test_unflagged_tools_are_unaffected(tmp_path):
    agent = make_agent(tmp_path, [TIME_ACTION, "FINAL: told the time"])
    task = Task(
        description="what time is it",
        department="research",
        needs_approval=["calculator"],
        approved=False,
    )
    result = agent.run(task)
    assert result.tool_calls[0].tool == "current_time"
    assert result.tool_calls[0].success is True
    assert result.output == "told the time"


def test_no_wall_by_default(tmp_path):
    task = Task(description="plain task")
    assert task.needs_approval == []
    assert task.approved is False
    agent = make_agent(tmp_path, [CALC_ACTION, "FINAL: 42"])
    result = agent.run(task)
    assert result.tool_calls[0].success is True


def test_blocked_call_is_recorded_in_the_ledger(tmp_path):
    ledger = Ledger()
    agent = make_agent(tmp_path, [CALC_ACTION, "FINAL: stopped"])
    task = Task(
        description="what is 6 times 7",
        department="research",
        needs_approval=["calculator"],
    )
    agent.run(task, ledger=ledger, run_id="wall-run")
    kinds = [event["kind"] for event in ledger.events("wall-run")]
    assert "tool_blocked" in kinds
    blocked = [
        event for event in ledger.events("wall-run") if event["kind"] == "tool_blocked"
    ]
    assert blocked[0]["payload"]["tool"] == "calculator"


def test_blocked_calls_do_not_trigger_same_error_escalation(tmp_path):
    agent = make_agent(
        tmp_path, [CALC_ACTION, CALC_ACTION, "FINAL: gave up politely"]
    )
    task = Task(
        description="what is 6 times 7",
        department="research",
        needs_approval=["calculator"],
    )
    result = agent.run(task)
    assert result.success is True
    assert not result.metadata.get("escalated")
    assert len(result.tool_calls) == 2


def test_plan_carries_the_wall_to_tasks():
    ceo = CEO(AgentRegistry())
    tasks = ceo.plan(
        "Research the history of the abacus",
        needs_approval=["shell"],
        approved=False,
    )
    assert tasks
    assert all(task.needs_approval == ["shell"] for task in tasks)
    assert all(task.approved is False for task in tasks)


def test_ask_endpoint_accepts_the_wall_fields():
    resp = client.post(
        "/ask",
        json={
            "request": "Research the history of the abacus",
            "needs_approval": ["shell"],
            "approved": False,
        },
    )
    assert resp.status_code == 200
