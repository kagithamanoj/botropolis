"""Trace tests: a run reads back as an ordered trace, in text and
JSON, and a missing run errors cleanly."""

import json

import pytest

from botropolis.cli import main
from botropolis.core.ledger import Ledger
from botropolis.core.runstate import RunStore
from botropolis.core.trace import build_trace, render_text


def _state(**overrides):
    state = {
        "run_id": "run1",
        "agent": "TestAgent",
        "task": {"description": "Work out some sums"},
        "contract": None,
        "steps": [
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
        "plan": "PLAN: Work out 6 times 7.\nThen work out 1 over 0.",
        "status": "finished",
        "answer": "42, and 1 over 0 has no answer.",
        "reason": None,
    }
    state.update(overrides)
    return state


def test_build_trace_from_state(tmp_path):
    store = RunStore(tmp_path / "runs")
    store.save(_state())
    trace = build_trace(store, "run1")
    assert trace["run_id"] == "run1"
    assert trace["agent"] == "TestAgent"
    assert trace["task"] == "Work out some sums"
    assert trace["status"] == "finished"
    assert trace["plan"] == ["Work out 6 times 7.", "Then work out 1 over 0."]
    assert len(trace["steps"]) == 2
    assert trace["steps"][0]["tool"] == "calculator"
    assert trace["steps"][0]["success"] is True
    assert trace["steps"][0]["outcome"] == "42"
    assert trace["steps"][1]["success"] is False
    assert trace["answer"] == "42, and 1 over 0 has no answer."
    assert trace["events"] == []


def test_build_trace_missing_run_raises(tmp_path):
    store = RunStore(tmp_path / "runs")
    with pytest.raises(FileNotFoundError):
        build_trace(store, "nope")


def test_build_trace_includes_ledger_events(tmp_path):
    store = RunStore(tmp_path / "runs")
    store.save(_state())
    ledger_path = tmp_path / "ledger.db"
    ledger = Ledger(ledger_path)
    ledger.record("run1", "plan_recorded", "TestAgent", {"plan": "PLAN: x"})
    ledger.record("run1", "tool_called", "TestAgent", {"tool": "calculator"})
    ledger.close()
    trace = build_trace(store, "run1", ledger_path=ledger_path)
    assert [event["kind"] for event in trace["events"]] == [
        "plan_recorded",
        "tool_called",
    ]
    assert trace["events"][0]["actor"] == "TestAgent"


def test_build_trace_skips_missing_ledger(tmp_path, monkeypatch):
    store = RunStore(tmp_path / "runs")
    store.save(_state())
    missing = tmp_path / "ledger.db"
    monkeypatch.setenv("BOTROPOLIS_LEDGER_DB", str(missing))
    trace = build_trace(store, "run1")
    assert trace["events"] == []
    assert not missing.is_file()


def test_render_text():
    trace = {
        "run_id": "run1",
        "agent": "TestAgent",
        "task": "Work out some sums",
        "status": "finished",
        "reason": "",
        "plan": ["Work out 6 times 7.", "Then work out 1 over 0."],
        "steps": [
            {
                "tool": "calculator",
                "args": {"expression": "6 * 7"},
                "success": True,
                "outcome": "42",
            },
            {
                "tool": "calculator",
                "args": {"expression": "1 / 0"},
                "success": False,
                "outcome": "ZeroDivisionError",
            },
        ],
        "events": [
            {
                "seq": 1,
                "kind": "plan_recorded",
                "actor": "TestAgent",
                "payload": {"plan": "PLAN: x"},
            }
        ],
        "answer": "42, and 1 over 0 has no answer.",
    }
    text = render_text(trace)
    assert "Trace: run1" in text
    assert "Agent: TestAgent" in text
    assert "Status: finished" in text
    assert "1. Work out 6 times 7." in text
    assert "calculator {'expression': '6 * 7'} -> ok: 42" in text
    assert "FAILED" in text
    assert "plan_recorded" in text
    assert "Answer:" in text


def test_cli_trace_text_and_json(tmp_path, capsys, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_RUNS_DIR", str(tmp_path / "runs"))
    store = RunStore(tmp_path / "runs")
    store.save(_state())
    assert main(["trace", "run1"]) == 0
    out = capsys.readouterr().out
    assert "Trace: run1" in out
    assert "Steps:" in out
    assert "calculator" in out
    assert main(["trace", "run1", "--json"]) == 0
    data = json.loads(capsys.readouterr().out)
    assert data["run_id"] == "run1"
    assert len(data["steps"]) == 2
    assert data["status"] == "finished"


def test_cli_trace_missing_run(tmp_path, capsys, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_RUNS_DIR", str(tmp_path / "runs"))
    assert main(["trace", "nope"]) == 1
    assert "No saved run with id nope" in capsys.readouterr().out
