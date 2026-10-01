"""Tests for the Gmail and Google Calendar tools.

Real mailbox writes are never exercised: CLI calls are stubbed out,
and the one live test only runs a read-only search against the
connected account.
"""

import json

import pytest

import botropolis.tools.gws as gws
from botropolis.core.registry import AgentRegistry
from botropolis.tools import TOOLS, get_tool


def test_gws_tools_registered_with_valid_schemas():
    for name in (
        "gmail_search",
        "gmail_read",
        "gmail_draft",
        "calendar_agenda",
        "calendar_create_event",
    ):
        tool = get_tool(name)
        assert tool.name == name
        assert isinstance(tool.description, str) and tool.description
        assert isinstance(tool.parameters, dict)
    assert set(TOOLS) >= {
        "gmail_search",
        "gmail_read",
        "gmail_draft",
        "calendar_agenda",
        "calendar_create_event",
    }


def test_gmail_search_parses_triage_json(monkeypatch):
    payload = {
        "messages": [
            {
                "id": "abc123",
                "from": "Ada <ada@example.com>",
                "subject": "Q3 report",
                "date": "Wed, 30 Sep 2026 10:00:00 +0000",
            }
        ]
    }
    monkeypatch.setattr(gws, "_run_cli", lambda args: {"data": payload})
    result = gws.GmailSearchTool().execute(query="subject:report")
    assert result["query"] == "subject:report"
    assert result["results"] == [
        {
            "id": "abc123",
            "from": "Ada <ada@example.com>",
            "subject": "Q3 report",
            "date": "Wed, 30 Sep 2026 10:00:00 +0000",
        }
    ]


def test_gmail_search_empty_result_is_not_an_error(monkeypatch):
    monkeypatch.setattr(
        gws, "_run_cli", lambda args: {"data": "No messages found matching query: xyz"}
    )
    result = gws.GmailSearchTool().execute(query="xyz")
    assert result["results"] == []
    assert "note" in result


def test_gmail_search_caps_max_results(monkeypatch):
    seen = {}

    def fake(args):
        seen["args"] = args
        return {"data": {"messages": []}}

    monkeypatch.setattr(gws, "_run_cli", fake)
    gws.GmailSearchTool().execute(query="x", max_results=500)
    assert "--max" in seen["args"]
    assert seen["args"][seen["args"].index("--max") + 1] == "20"


def test_gmail_read_extracts_fields(monkeypatch):
    monkeypatch.setattr(
        gws,
        "_run_cli",
        lambda args: {
            "data": {
                "from": "Ada <ada@example.com>",
                "subject": "Q3 report",
                "date": "Wed, 30 Sep 2026 10:00:00 +0000",
                "body": "Here are the numbers...",
            }
        },
    )
    result = gws.GmailReadTool().execute(message_id="abc123")
    assert result["from"] == "Ada <ada@example.com>"
    assert result["subject"] == "Q3 report"
    assert "numbers" in result["body"]


def test_gmail_draft_rejects_bad_address_without_calling_cli(monkeypatch):
    def fail(args):
        raise AssertionError("CLI must not be called")

    monkeypatch.setattr(gws, "_run_cli", fail)
    result = gws.GmailDraftTool().execute(to="not-an-address", subject="s", body="b")
    assert "error" in result


def test_gmail_draft_passes_through_to_cli(monkeypatch):
    seen = {}

    def fake(args):
        seen["args"] = args
        return {"data": {"id": "draft1"}}

    monkeypatch.setattr(gws, "_run_cli", fake)
    result = gws.GmailDraftTool().execute(
        to="ada@example.com", subject="Hi", body="Hello"
    )
    assert result["drafted"] is True
    assert seen["args"][:2] == ["gmail", "+draft"]
    assert "--to" in seen["args"] and "--subject" in seen["args"]


def test_calendar_agenda_degrades_when_not_connected(monkeypatch):
    monkeypatch.setattr(
        gws,
        "_run_cli",
        lambda args: {"error": "not_connected", "detail": "connect it first"},
    )
    result = gws.CalendarAgendaTool().execute(days=7)
    assert result["error"] == "not_connected"


def test_calendar_create_event_never_sends_attendees(monkeypatch):
    seen = {}

    def fake(args):
        seen["args"] = args
        return {"data": {"event_starts_at": {"user_local": "2026-10-02T14:00:00-05:00"}}}

    monkeypatch.setattr(gws, "_run_cli", fake)
    result = gws.CalendarCreateEventTool().execute(
        summary="Focus block",
        start="2026-10-02T14:00:00-05:00",
        end="2026-10-02T15:00:00-05:00",
    )
    assert result["created"] is True
    body = json.loads(seen["args"][seen["args"].index("--json") + 1])
    assert "attendees" not in body
    assert body["summary"] == "Focus block"


def test_missing_cli_degrades_gracefully(monkeypatch):
    monkeypatch.setattr(gws.shutil, "which", lambda name: None)
    result = gws.GmailSearchTool().execute(query="x")
    assert "error" in result
    assert "not installed" in result["error"]


def test_ops_agents_carry_the_new_toolkits():
    registry = AgentRegistry()
    anya = registry.get("Anya")
    assert anya.toolkit == [
        "gmail_search",
        "gmail_read",
        "gmail_draft",
        "notes_read",
        "notes_append",
    ]
    tyler = registry.get("Tyler")
    assert tyler.toolkit == [
        "calendar_agenda",
        "calendar_create_event",
        "current_time",
        "notes_read",
        "notes_append",
    ]


def test_gmail_search_live_read_only():
    """Read-only check against the connected mailbox. Skipped offline."""
    if gws.shutil.which(gws._CLI) is None:
        pytest.skip("hatch_gws_cli not installed")
    result = gws.GmailSearchTool().execute(query="in:inbox", max_results=1)
    if result.get("error") == "not_connected":
        pytest.skip("Gmail not connected")
    assert "error" not in result
    assert isinstance(result["results"], list)
