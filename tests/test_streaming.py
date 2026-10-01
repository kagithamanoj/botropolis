"""Tests for streaming tool-call updates: loop events and the SSE endpoint."""

import yaml

from fastapi.testclient import TestClient

from botropolis.core.agent import Agent
from botropolis.core.loop import run_tool_loop
from botropolis.core.schemas import Task
from botropolis.server import _agent_event_stream, app

client = TestClient(app)


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


def make_agent(tmp_path, toolkit=None, replies=None):
    spec = {
        "name": "StreamAgent",
        "title": "Streamer",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
    }
    if toolkit is not None:
        spec["toolkit"] = toolkit
    path = tmp_path / "streamagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=FakeClient(replies or ["FINAL: done"]))


def collect_events(agent, request_text="do a thing"):
    """Drain _agent_event_stream into a list of (kind, payload) tuples."""
    import json

    chunks = "".join(_agent_event_stream(agent, request_text))
    events = []
    for block in chunks.strip().split("\n\n"):
        lines = block.splitlines()
        kind = lines[0].split("event: ", 1)[1]
        payload = json.loads(lines[1].split("data: ", 1)[1])
        events.append((kind, payload))
    return events


def test_loop_emits_tool_events_in_order(tmp_path):
    replies = [
        'ACTION: {"tool": "calculator", "args": {"expression": "2 + 3"}}',
        'ACTION: {"tool": "calculator", "args": {"expression": "5 * 2"}}',
        "FINAL: The answer is 10",
    ]
    agent = make_agent(tmp_path, toolkit=["calculator"], replies=replies)
    seen = []
    task = Task(description="multiply", department="research", agent_name="StreamAgent")
    output, tool_calls, _ = run_tool_loop(
        agent, task, on_event=lambda kind, payload: seen.append((kind, payload))
    )
    assert output == "The answer is 10"
    assert [k for k, _ in seen] == [
        "tool_started",
        "tool_finished",
        "tool_started",
        "tool_finished",
    ]
    assert seen[0][1]["tool"] == "calculator"
    assert seen[0][1]["args"] == {"expression": "2 + 3"}
    assert seen[1][1]["success"] is True
    assert len(tool_calls) == 2


def test_loop_without_on_event_still_works(tmp_path):
    agent = make_agent(tmp_path, toolkit=["calculator"],
                       replies=["FINAL: plain answer"])
    task = Task(description="hi", department="research", agent_name="StreamAgent")
    output, tool_calls, _ = run_tool_loop(agent, task)
    assert output == "plain answer"
    assert tool_calls == []


def test_raising_on_event_does_not_break_loop(tmp_path):
    def bad(kind, payload):
        raise RuntimeError("listener blew up")

    agent = make_agent(
        tmp_path,
        toolkit=["calculator"],
        replies=[
            'ACTION: {"tool": "calculator", "args": {"expression": "1 + 1"}}',
            "FINAL: two",
        ],
    )
    output, tool_calls, _ = run_tool_loop(
        agent,
        Task(description="add", department="research", agent_name="StreamAgent"),
        on_event=bad,
    )
    assert output == "two"
    assert len(tool_calls) == 1


def test_stream_endpoint_emits_result_and_done():
    resp = client.get("/agents/Liam/ask/stream", params={"request": "hello"})
    assert resp.status_code == 200
    assert "text/event-stream" in resp.headers["content-type"]
    kinds = [
        line.split("event: ", 1)[1]
        for line in resp.text.splitlines()
        if line.startswith("event: ")
    ]
    assert "result" in kinds
    assert kinds[-1] == "done"
    import json

    for block in resp.text.strip().split("\n\n"):
        lines = block.splitlines()
        if lines[0] == "event: result":
            payload = json.loads(lines[1].split("data: ", 1)[1])
            assert payload["agent_name"] == "Liam"
            assert payload["success"] is True


def test_stream_endpoint_unknown_agent_404():
    resp = client.get("/agents/NobodyHere/ask/stream", params={"request": "hi"})
    assert resp.status_code == 404


def test_stream_endpoint_rejects_empty_request():
    resp = client.get("/agents/Liam/ask/stream", params={"request": ""})
    assert resp.status_code == 422


def test_stream_events_flow_end_to_end(tmp_path):
    replies = [
        'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}',
        "FINAL: 42",
    ]
    agent = make_agent(tmp_path, toolkit=["calculator"], replies=replies)
    events = collect_events(agent, "multiply six by seven")
    kinds = [k for k, _ in events]
    assert kinds == ["tool_started", "tool_finished", "result", "done"]
    started = events[0][1]
    assert started["tool"] == "calculator"
    assert started["args"] == {"expression": "6 * 7"}
    finished = events[1][1]
    assert finished["success"] is True
    result = events[2][1]
    assert result["output"] == "42"
    assert len(result["tool_calls"]) == 1
