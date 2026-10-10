"""Ledger tests: the append-only event trail behind every run."""

import json

import pytest
import yaml

from botropolis.core.agent import Agent
from botropolis.core.ledger import Ledger, new_run_id
from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry


class FakeClient:
    def __init__(self, replies, provider="openai"):
        self.replies = list(replies)
        self.provider = provider
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return self.provider


def make_agent(tmp_path, replies, provider="openai", toolkit=None):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "ops",
        "specialty": "testing",
        "model": "gpt-4o",
        "system_prompt": "You are a test agent.",
    }
    if toolkit is not None:
        spec["toolkit"] = toolkit
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=FakeClient(replies, provider=provider))


def good_reply(summary="Did the thing."):
    return json.dumps({"summary": summary, "details": "", "key_points": []})


# ── ledger mechanics ────────────────────────────────────────────────────

def test_record_and_replay_round_trip():
    ledger = Ledger()
    run_id = new_run_id()
    ledger.record(run_id, "task_assigned", "alice", {"task": "x"})
    ledger.record(run_id, "output_produced", "alice", {"summary": "done"})
    events = ledger.events(run_id)
    assert [e["kind"] for e in events] == ["task_assigned", "output_produced"]
    assert [e["seq"] for e in events] == [1, 2]
    assert all(e["actor"] == "alice" for e in events)
    assert events[0]["payload"] == {"task": "x"}
    assert all(e["ts"] > 0 for e in events)


def test_events_are_isolated_per_run():
    ledger = Ledger()
    a, b = new_run_id(), new_run_id()
    ledger.record(a, "x", "alice")
    ledger.record(b, "y", "bob")
    assert [e["kind"] for e in ledger.events(a)] == ["x"]
    assert [e["kind"] for e in ledger.events(b)] == ["y"]


def test_runs_lists_runs_newest_first():
    ledger = Ledger()
    a, b = new_run_id(), new_run_id()
    ledger.record(a, "run_started", "ceo")
    ledger.record(a, "run_finished", "ceo")
    ledger.record(b, "run_started", "ceo")
    runs = ledger.runs()
    assert [r["run_id"] for r in runs] == [b, a]
    assert runs[1]["events"] == 2


def test_file_backed_ledger_persists(tmp_path):
    path = str(tmp_path / "runs.db")
    run_id = new_run_id()
    ledger = Ledger(path)
    ledger.record(run_id, "task_assigned", "alice", {"n": 1})
    ledger.close()
    reopened = Ledger(path)
    events = reopened.events(run_id)
    assert len(events) == 1
    assert events[0]["payload"] == {"n": 1}
    reopened.close()


def test_run_ids_are_unique():
    assert len({new_run_id() for _ in range(100)}) == 100


# ── agent integration ───────────────────────────────────────────────────

def test_agent_run_records_task_and_output(tmp_path):
    agent = make_agent(tmp_path, [good_reply()])
    ledger = Ledger()
    run_id = new_run_id()
    result = agent.run("Do the thing", ledger=ledger, run_id=run_id)
    assert result.success
    kinds = [e["kind"] for e in ledger.events(run_id)]
    assert kinds == ["task_assigned", "output_produced"]
    produced = ledger.events(run_id)[1]
    assert produced["actor"] == "TestAgent"
    assert produced["payload"]["summary"] == "Did the thing."
    assert produced["payload"]["structured"]["summary"] == "Did the thing."


def test_agent_run_without_ledger_is_unchanged(tmp_path):
    agent = make_agent(tmp_path, [good_reply()])
    result = agent.run("Do the thing")
    assert result.success
    assert result.structured["summary"] == "Did the thing."


def test_repair_records_output_repair_event(tmp_path):
    bad = json.dumps({"details": "no summary"})
    agent = make_agent(tmp_path, [bad, good_reply()])
    ledger = Ledger()
    run_id = new_run_id()
    result = agent.run("Do the thing", ledger=ledger, run_id=run_id)
    assert result.success
    kinds = [e["kind"] for e in ledger.events(run_id)]
    assert kinds == ["task_assigned", "output_repair", "output_produced"]
    repair = ledger.events(run_id)[1]
    assert "summary" in repair["payload"]["error"]


def test_failed_contract_records_agent_failed(tmp_path):
    agent = make_agent(tmp_path, ["nope", "still nope"])
    ledger = Ledger()
    run_id = new_run_id()
    result = agent.run("Do the thing", ledger=ledger, run_id=run_id)
    assert not result.success
    kinds = [e["kind"] for e in ledger.events(run_id)]
    assert kinds == ["task_assigned", "output_repair", "agent_failed"]


def test_tool_calls_are_recorded(tmp_path):
    agent = make_agent(
        tmp_path,
        [
            'ACTION: {"tool": "calculator", "args": {"expression": "1 + 1"}}',
            f'FINAL: {good_reply("2")}',
        ],
        toolkit=["calculator"],
    )
    ledger = Ledger()
    run_id = new_run_id()
    result = agent.run("Add", ledger=ledger, run_id=run_id)
    assert result.success
    kinds = [e["kind"] for e in ledger.events(run_id)]
    assert kinds == ["task_assigned", "tool_called", "output_produced"]
    tool_event = ledger.events(run_id)[1]
    assert tool_event["payload"]["tool"] == "calculator"
    assert tool_event["payload"]["success"] is True


def test_broken_ledger_never_breaks_the_run(tmp_path):
    class BrokenLedger:
        def record(self, *a, **k):
            raise OSError("disk is gone")

    agent = make_agent(tmp_path, [good_reply()])
    result = agent.run("Do the thing", ledger=BrokenLedger(), run_id="r1")
    assert result.success


def test_ledger_mints_run_id_when_missing(tmp_path):
    agent = make_agent(tmp_path, [good_reply()])
    ledger = Ledger()
    agent.run("Do the thing", ledger=ledger)
    runs = ledger.runs()
    assert len(runs) == 1
    assert [e["kind"] for e in ledger.events(runs[0]["run_id"])] == [
        "task_assigned",
        "output_produced",
    ]


# ── CEO integration ─────────────────────────────────────────────────────

def test_ceo_handle_records_full_trail():
    ledger = Ledger()
    ceo = CEO(AgentRegistry(), ledger=ledger)
    report = ceo.handle("What is 6 times 7?")
    assert report.results
    runs = ledger.runs()
    assert len(runs) == 1
    kinds = [e["kind"] for e in ledger.events(runs[0]["run_id"])]
    assert kinds[0] == "run_started"
    assert kinds[-1] == "run_finished"
    assert "task_assigned" in kinds
    assert "output_produced" in kinds
    finished = ledger.events(runs[0]["run_id"])[-1]
    assert finished["actor"] == "Manoj"
    assert finished["payload"]["successful"] is True


def test_ceo_team_records_full_trail():
    ledger = Ledger()
    ceo = CEO(AgentRegistry(), ledger=ledger)
    report = ceo.team("Draft a checklist", ["scout", "scheduler"], rounds=1)
    assert len(report.rounds) == 2
    kinds = [e["kind"] for e in ledger.events(ledger.runs()[0]["run_id"])]
    assert kinds.count("task_assigned") == 2
    assert kinds.count("output_produced") == 2
    assert kinds[-1] == "run_finished"


def test_ceo_defaults_to_in_memory_ledger():
    ceo = CEO(AgentRegistry())
    ceo.handle("Hello")
    assert len(ceo.ledger.runs()) == 1
