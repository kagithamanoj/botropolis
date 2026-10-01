"""Gmail and Google Calendar tools for Botropolis agents.

These shell out to the hatch_gws_cli commands from the gmail and
google-calendar skills. Everything degrades gracefully: when a service
is not connected, or the CLI is missing, the tool returns a plain
message instead of raising, so the agent loop keeps working.

Deliberate limits:
- gmail_draft creates drafts only. There is no gmail_send tool: agents
  prepare mail, humans send it.
- calendar_create_event creates private events only (no attendees), so
  nothing ever notifies another person without a human in the loop.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from typing import Any, Dict, List

from botropolis.tools.builtin import Tool

_CLI = "hatch_gws_cli"
_TIMEOUT = 90


def _run_cli(args: List[str]) -> Dict[str, Any]:
    """Run one hatch_gws_cli command.

    Returns {"data": parsed} on success or {"error": message} on any
    failure. Never raises.
    """
    exe = shutil.which(_CLI)
    if exe is None:
        return {"error": f"{_CLI} is not installed on this machine."}
    try:
        proc = subprocess.run(
            [exe] + args, capture_output=True, text=True, timeout=_TIMEOUT
        )
    except subprocess.TimeoutExpired:
        return {"error": "The command timed out."}
    except OSError as exc:
        return {"error": f"Could not run {_CLI}: {exc}"}
    out = (proc.stdout or "").strip()
    if proc.returncode != 0:
        err = (proc.stderr or "").strip() or out or "unknown error"
        lowered = err.lower()
        if "not connected" in lowered or "missing access_token" in lowered:
            return {
                "error": "not_connected",
                "detail": (
                    "This Google service is not connected on this machine. "
                    "Tell the user to connect it before asking an agent to use it."
                ),
            }
        return {"error": f"Command failed: {err[:300]}"}
    if not out:
        return {"error": "The command returned nothing."}
    try:
        return {"data": json.loads(out)}
    except json.JSONDecodeError:
        # Plain-text answers, e.g. "No messages found matching query: ...".
        return {"data": out}


def _short(text: Any, limit: int = 3000) -> str:
    text = "" if text is None else str(text)
    text = text.strip()
    if len(text) > limit:
        text = text[:limit] + "... [truncated]"
    return text


class GmailSearchTool(Tool):
    """Search the connected Gmail mailbox."""

    name = "gmail_search"
    description = (
        "Search Gmail with a Gmail search query (from:, subject:, "
        "is:unread, newer_than:7d, ...). Returns sender, subject, date, "
        "and message id for each match. Read-only."
    )
    parameters = {
        "query": {"type": "string", "description": "Gmail search query", "required": True},
        "max_results": {"type": "integer", "description": "Max results", "default": 5},
    }

    def execute(self, query: str, max_results: int = 5) -> Dict[str, Any]:
        max_results = max(1, min(int(max_results), 20))
        res = _run_cli(
            ["gmail", "+triage", "--query", query,
             "--max", str(max_results), "--format", "json"]
        )
        if "error" in res:
            return res
        data = res["data"]
        if isinstance(data, str):
            return {"query": query, "results": [], "note": data}
        messages = data.get("messages", []) if isinstance(data, dict) else []
        return {
            "query": query,
            "results": [
                {
                    "id": m.get("id"),
                    "from": m.get("from"),
                    "subject": m.get("subject"),
                    "date": m.get("date"),
                }
                for m in messages
            ],
        }


class GmailReadTool(Tool):
    """Read one Gmail message by id."""

    name = "gmail_read"
    description = (
        "Read a single Gmail message by its id (from gmail_search). "
        "Returns sender, subject, date, and a body excerpt. Read-only."
    )
    parameters = {
        "message_id": {"type": "string", "description": "Gmail message id", "required": True},
    }

    def execute(self, message_id: str) -> Dict[str, Any]:
        res = _run_cli(
            ["gmail", "+read", "--id", message_id, "--headers", "--format", "json"]
        )
        if "error" in res:
            return res
        data = res["data"]
        if isinstance(data, str):
            return {"message_id": message_id, "body": _short(data)}
        if not isinstance(data, dict):
            return {"message_id": message_id, "body": _short(data)}
        body = (
            data.get("body")
            or data.get("snippet")
            or data.get("text")
            or json.dumps(data, default=str)
        )
        return {
            "message_id": message_id,
            "from": data.get("from") or data.get("From"),
            "subject": data.get("subject") or data.get("Subject"),
            "date": data.get("date") or data.get("Date"),
            "body": _short(body),
        }


class GmailDraftTool(Tool):
    """Create a Gmail draft. Drafts only: nothing is ever sent."""

    name = "gmail_draft"
    description = (
        "Create a Gmail draft with to, subject, and body. The draft is "
        "saved to Drafts for a human to review and send. This tool "
        "never sends mail."
    )
    parameters = {
        "to": {"type": "string", "description": "Recipient email address", "required": True},
        "subject": {"type": "string", "description": "Email subject", "required": True},
        "body": {"type": "string", "description": "Email body, plain text", "required": True},
    }

    def execute(self, to: str, subject: str, body: str) -> Dict[str, Any]:
        if not to or "@" not in to:
            return {"error": f"Not a valid recipient address: {to!r}"}
        res = _run_cli(
            ["gmail", "+draft", "--to", to, "--subject", subject, "--body", body]
        )
        if "error" in res:
            return res
        return {"drafted": True, "to": to, "subject": subject, "detail": res["data"]}


class CalendarAgendaTool(Tool):
    """List upcoming Google Calendar events."""

    name = "calendar_agenda"
    description = (
        "Show upcoming events on the connected Google Calendar for the "
        "next N days. Read-only."
    )
    parameters = {
        "days": {"type": "integer", "description": "Days ahead to look", "default": 7},
    }

    def execute(self, days: int = 7) -> Dict[str, Any]:
        days = max(1, min(int(days), 31))
        res = _run_cli(["calendar", "+agenda", "--days", str(days), "--format", "json"])
        if "error" in res:
            return res
        data = res["data"]
        if isinstance(data, str):
            return {"days": days, "events": [], "note": data}
        events = data.get("events", []) if isinstance(data, dict) else []
        return {
            "days": days,
            "events": [
                {
                    "summary": e.get("summary"),
                    "starts_at": e.get("event_starts_at") or e.get("start"),
                    "ends_at": e.get("event_ends_at") or e.get("end"),
                    "location": e.get("location"),
                }
                for e in events
            ],
        }


class CalendarCreateEventTool(Tool):
    """Create a private calendar event. No attendees, no notifications."""

    name = "calendar_create_event"
    description = (
        "Create a private event on the primary Google Calendar. "
        "Start and end are RFC3339 datetimes with a timezone offset, "
        "e.g. 2026-10-02T14:00:00-05:00. No attendees are invited, so "
        "nobody else is notified."
    )
    parameters = {
        "summary": {"type": "string", "description": "Event title", "required": True},
        "start": {"type": "string", "description": "Start datetime, RFC3339", "required": True},
        "end": {"type": "string", "description": "End datetime, RFC3339", "required": True},
        "description": {"type": "string", "description": "Event description", "required": False},
        "location": {"type": "string", "description": "Event location", "required": False},
    }

    def execute(
        self,
        summary: str,
        start: str,
        end: str,
        description: str = "",
        location: str = "",
    ) -> Dict[str, Any]:
        event: Dict[str, Any] = {
            "summary": summary,
            "start": {"dateTime": start},
            "end": {"dateTime": end},
        }
        if description:
            event["description"] = description
        if location:
            event["location"] = location
        res = _run_cli(
            [
                "calendar", "events", "insert",
                "--params", json.dumps({"calendarId": "primary"}),
                "--json", json.dumps(event),
            ]
        )
        if "error" in res:
            return res
        data = res["data"]
        created = data if isinstance(data, dict) else {}
        return {
            "created": True,
            "summary": summary,
            "starts_at": created.get("event_starts_at"),
            "ends_at": created.get("event_ends_at"),
        }
