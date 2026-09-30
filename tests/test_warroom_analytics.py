"""Tests for the war room endpoint, usage logging, and the analytics endpoint."""

from fastapi.testclient import TestClient

from botropolis.core import usage
from botropolis.core.models import ModelClient
from botropolis.server import app

client = TestClient(app)


def test_stub_reports_zero_token_usage():
    c = ModelClient()
    text = c.chat("stub", [{"role": "user", "content": "hello"}])
    assert text
    assert c.last_usage == {"input": 0, "output": 0}


def test_war_room_ask_returns_agent_result(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "usage.jsonl")
    resp = client.post("/agents/Scout/ask", json={"request": "what is TCP?"})
    assert resp.status_code == 200
    body = resp.json()
    assert body["agent_name"] == "Scout"
    assert body["success"] is True
    assert body["output"]
    assert body["metadata"]["tokens_in"] == 0
    assert body["metadata"]["tokens_out"] == 0


def test_war_room_ask_unknown_agent_404():
    resp = client.post("/agents/Nope/ask", json={"request": "hello"})
    assert resp.status_code == 404


def test_war_room_ask_logs_usage(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "usage.jsonl")
    client.post("/agents/Coder/ask", json={"request": "write a loop"})
    summary = usage.get_summary()
    assert summary["agents"]["Coder"]["calls"] == 1
    assert summary["totals"]["stub_calls"] == 1


def test_ask_logs_usage(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "usage.jsonl")
    resp = client.post("/ask", json={"request": "explain TCP"})
    assert resp.status_code == 200
    summary = usage.get_summary()
    assert summary["totals"]["calls"] >= 1


def test_usage_summary_aggregates(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "usage.jsonl")
    usage.log_invocation("Scout", "research", "stub", "stub", 12.5, 0, 0, True)
    usage.log_invocation("Scout", "research", "stub", "stub", 7.5, 0, 0, True)
    usage.log_invocation("Coder", "code", "gpt-4o", "openai", 100.0, 50, 20, True)
    summary = usage.get_summary()
    assert summary["agents"]["Scout"]["calls"] == 2
    assert summary["agents"]["Scout"]["avg_latency_ms"] == 10.0
    assert summary["agents"]["Coder"]["tokens_total"] == 70
    assert summary["totals"]["calls"] == 3
    assert summary["totals"]["stub_calls"] == 2


def test_usage_summary_empty_log(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "missing.jsonl")
    summary = usage.get_summary()
    assert summary["agents"] == {}
    assert summary["totals"]["calls"] == 0


def test_analytics_endpoint(monkeypatch, tmp_path):
    monkeypatch.setattr(usage, "LOG_PATH", tmp_path / "usage.jsonl")
    resp = client.get("/analytics")
    assert resp.status_code == 200
    body = resp.json()
    assert "agents" in body
    assert "totals" in body
