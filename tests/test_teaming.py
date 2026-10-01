"""Tests for agent teaming: CEO.team() and the POST /team endpoint."""

import pytest
from fastapi.testclient import TestClient

from botropolis.core import usage
from botropolis.core.orchestrator import CEO, MAX_TEAM_ROUNDS
from botropolis.server import app

client = TestClient(app)


def test_team_runs_agents_in_order():
    ceo = CEO()
    report = ceo.team("write a python function", ["Liam", "Sofia"], rounds=1)
    assert report.agents == ["Liam", "Sofia"]
    assert [r.agent_name for r in report.rounds] == ["Liam", "Sofia"]
    assert [r.round_number for r in report.rounds] == [1, 1]
    assert report.successful()


def test_later_rounds_see_earlier_outputs():
    ceo = CEO()
    report = ceo.team("write a python function", ["Liam", "Sofia"], rounds=2)
    assert len(report.rounds) == 4
    round2_coder = [
        r for r in report.rounds if r.round_number == 2 and r.agent_name == "Liam"
    ][0]
    # The stub restates its full prompt, so round 1 output must show up in it.
    assert "[round 1 - Liam]" in round2_coder.result.output
    assert "[round 1 - Sofia]" in round2_coder.result.output


def test_team_unknown_agent_raises():
    ceo = CEO()
    with pytest.raises(KeyError, match="Unknown agent"):
        ceo.team("hello", ["Liam", "Nobody"], rounds=1)


def test_team_needs_at_least_one_agent():
    ceo = CEO()
    with pytest.raises(ValueError):
        ceo.team("hello", [], rounds=1)


def test_team_rounds_capped():
    ceo = CEO()
    report = ceo.team("hello", ["Ethan"], rounds=99)
    assert max(r.round_number for r in report.rounds) == MAX_TEAM_ROUNDS
    assert len(report.rounds) == MAX_TEAM_ROUNDS


def test_team_synthesis_covers_final_state():
    ceo = CEO()
    report = ceo.team("draft a memo", ["Zara"], rounds=1)
    assert report.synthesis
    assert "Zara" in report.synthesis


def test_team_endpoint_shape():
    resp = client.post(
        "/team",
        json={"request": "write a function", "agents": ["Liam", "Sofia"], "rounds": 1},
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["request"] == "write a function"
    assert body["agents"] == ["Liam", "Sofia"]
    assert len(body["rounds"]) == 2
    first = body["rounds"][0]
    assert first["round_number"] == 1
    assert first["agent_name"] == "Liam"
    assert first["result"]["success"] is True
    assert body["synthesis"]


def test_team_endpoint_unknown_agent_404():
    resp = client.post("/team", json={"request": "hi", "agents": ["Nobody"]})
    assert resp.status_code == 404


def test_team_endpoint_rejects_too_many_rounds():
    resp = client.post("/team", json={"request": "hi", "agents": ["Ethan"], "rounds": 99})
    assert resp.status_code == 422


def test_team_endpoint_logs_usage(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "usage.jsonl")
    resp = client.post(
        "/team", json={"request": "hi", "agents": ["Ethan", "Liam"], "rounds": 2}
    )
    assert resp.status_code == 200
    summary = usage.get_summary()
    assert summary["agents"]["Ethan"]["calls"] == 2
    assert summary["agents"]["Liam"]["calls"] == 2
