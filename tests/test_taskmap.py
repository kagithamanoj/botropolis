"""Task map tests: a run's progress in words, written after every
step, read back on resume, and printable from the CLI."""

import os

import pytest
import yaml

from botropolis.cli import main
from botropolis.core.agent import Agent
from botropolis.core.runstate import RunStore, task_from_state
from botropolis.core.schemas import Contract, Task
from botropolis.core.taskmap import (
    map_path,
    plan_steps,
    read_map,
    render_map,
)

CALC_6_7 = 'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}'
CALC_8_9 = 'ACTION: {"tool": "calculator", "args": {"expression": "8 * 9"}}'
PLAN = (
    "PLAN: Work out 6 times 7.\n"
    "Then work out 8 times 9.\n"
    "Then answer."
)


class ScriptedClient:
    """Replies play in order; records the messages it was shown."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}
        self.seen_messages = []

    def chat(self, model_id, messages):
        self.calls += 1
        self.seen_messages = messages
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


class CrashingClient:
    """Answers once with a tool call, then dies like a dead process."""

    def __init__(self):
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        if self.calls == 1:
            return CALC_6_7
        raise RuntimeError("process died")

    def provider_for(self, model_id):
        return "stub"


def make_agent(tmp_path, client, **spec_overrides):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
        "toolkit": ["calculator"],
    }
    spec.update(spec_overrides)
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=client)


def _state(**overrides):
    state = {
        "run_id": "abc123",
        "agent": "TestAgent",
        "task": {"description": "Work out some sums"},
        "contract": {
            "goal": "Work out some sums",
            "done_when": ["6 times 7 is 42", "8 times 9 is 72"],
        },
        "steps": [],
        "plan": None,
        "status": "running",
        "answer": None,
        "reason": None,
    }
    state.update(overrides)
    return state


def test_plan_steps_parsing():
    assert plan_steps(None) == []
    assert plan_steps("") == []
    assert plan_steps("PLAN: First do this.\nThen do that.") == [
        "First do this.",
        "Then do that.",
    ]
    assert plan_steps("PLAN: One step only") == ["One step only"]
    assert plan_steps("PLAN: 1. First step\n2. Second step\n- Third step") == [
        "First step",
        "Second step",
        "Third step",
    ]


def test_render_map_lists_goal_and_done_when():
    text = render_map(_state())
    assert "# Task map: abc123" in text
    assert "Task: Work out some sums" in text
    assert "## Goal" in text
    assert "Work out some sums" in text
    assert "- 6 times 7 is 42" in text
    assert "- 8 times 9 is 72" in text
    assert "none yet" in text


def test_render_map_marks_plan_steps_and_blockers():
    state = _state(
        plan=PLAN,
        steps=[
            {
                "tool": "calculator",
                "args": {"expression": "6 * 7"},
                "success": True,
                "result_preview": "42",
                "elapsed_ms": 1.0,
            },
            {
                "tool": "calculator",
                "args": {"expression": "1 / 0"},
                "success": False,
                "result_preview": "ZeroDivisionError: division by zero",
                "elapsed_ms": 1.0,
            },
        ],
        status="escalated",
        reason="tool 'calculator' failed twice",
    )
    text = render_map(state)
    assert "- [x] Work out 6 times 7." in text
    assert "- [ ] Then work out 8 times 9." in text
    assert "calculator {'expression': '6 * 7'} -> ok: 42" in text
    assert "## Blockers" in text
    assert "calculator failed: ZeroDivisionError: division by zero" in text
    assert "escalated: tool 'calculator' failed twice" in text


def test_map_written_after_steps(tmp_path):
    store = RunStore(tmp_path / "runs")
    client = ScriptedClient([PLAN, CALC_6_7, CALC_8_9, "FINAL: 42 and 72", "CHECK: PASS"])
    agent = make_agent(tmp_path, client)
    contract = Contract(
        goal="Work out some sums",
        done_when=["6 times 7 is 42", "8 times 9 is 72"],
    )
    result = agent.run(
        Task(description="Work out some sums", contract=contract),
        run_id="run1",
        store=store,
    )
    assert result.success
    path = map_path(store, "run1")
    assert path.is_file()
    text = path.read_text(encoding="utf-8")
    assert "Task: Work out some sums" in text
    assert "## Goal" in text
    assert "- [x] Work out 6 times 7." in text
    assert "- [x] Then work out 8 times 9." in text
    assert "- [ ] Then answer." in text
    assert "calculator {'expression': '6 * 7'} -> ok: 42" in text
    assert "calculator {'expression': '8 * 9'} -> ok: 72" in text
    assert "Status: finished" in text


def test_map_records_failed_call_as_blocker(tmp_path):
    store = RunStore(tmp_path / "runs")
    client = ScriptedClient(
        [
            'ACTION: {"tool": "calculator", "args": {"expression": "1 / 0"}}',
            "FINAL: could not work it out",
            "CHECK: PASS",
        ]
    )
    agent = make_agent(tmp_path, client)
    result = agent.run("Work out 1 over 0", run_id="run2", store=store)
    assert result.success
    text = read_map(store, "run2")
    assert "## Blockers" in text
    assert "FAILED" in text


def test_resume_attaches_map(tmp_path):
    store = RunStore(tmp_path / "runs")
    agent = make_agent(tmp_path, CrashingClient())
    first = agent.run("Work out 6 times 7", run_id="run3", store=store)
    assert not first.success
    assert map_path(store, "run3").is_file()
    state = store.load("run3")
    client = ScriptedClient(["FINAL: 42", "CHECK: PASS"])
    resumed = make_agent(tmp_path, client)
    result = resumed.run(
        task_from_state(state),
        run_id=state["run_id"],
        store=store,
        resume_state=state,
    )
    assert result.success
    shown = "\n".join(m.get("content", "") for m in client.seen_messages)
    assert "Task map so far:" in shown
    assert "calculator" in shown


def test_read_map_falls_back_to_state(tmp_path):
    store = RunStore(tmp_path / "runs")
    store.save(_state(run_id="run4"))
    assert not map_path(store, "run4").is_file()
    assert "Task map: run4" in read_map(store, "run4")


def test_cli_map_prints_and_errors(tmp_path, capsys, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_RUNS_DIR", str(tmp_path / "runs"))
    assert main(["map", "nope"]) == 1
    assert "No saved run with id nope" in capsys.readouterr().out
    store = RunStore(tmp_path / "runs")
    store.save(_state(run_id="run5"))
    assert main(["map", "run5"]) == 0
    assert "Task map: run5" in capsys.readouterr().out
