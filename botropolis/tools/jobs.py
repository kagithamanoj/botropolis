"""Job hunting tools backed by the RemoteOK public feed.

job_search reads the RemoteOK API (no key needed) and returns postings
that match a set of keywords. The feed's first element is metadata, not
a job, so it is skipped. A search that finds nothing is a normal empty
result, but a search that could not run is an error: network failures,
bad HTTP statuses, and malformed payloads all raise, so nobody mistakes
a broken feed for "no jobs today".

job_apply is deliberately narrower than its name. It does not submit
anything to an external site. It validates that the resume lives inside
the workspace, records the application in a local log
(botropolis/data/applications.jsonl), and returns the package, apply
url included, so a human or a browser flow does the actual submission.
Applying twice to the same role returns a warning instead of a second
record.

Tests and evals can point BOTROPOLIS_JOBS_FEED_FILE at a saved copy of
the feed payload to run without the network, and
BOTROPOLIS_APPLICATIONS_LOG moves the applications log.
"""

from __future__ import annotations

import json
import os
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List

import requests

from botropolis.tools.builtin import Tool
from botropolis.tools.sandbox import resolve_workspace_path, workspace_relative

REMOTEOK_API_URL = "https://remoteok.com/api"

_USER_AGENT = "botropolis job_search tool"

DEFAULT_LIMIT = 10
MAX_LIMIT = 25


def _fetch_feed() -> Any:
    """Return the parsed RemoteOK feed payload.

    Raises ValueError with a specific message on any failure: the tool
    contract is that a broken feed is loud, never a silent empty list.
    """
    feed_file = os.environ.get("BOTROPOLIS_JOBS_FEED_FILE")
    if feed_file:
        try:
            raw = Path(feed_file).read_text(encoding="utf-8")
        except OSError as exc:
            raise ValueError(
                f"job search failed: could not read feed file {feed_file}: {exc}"
            ) from exc
        try:
            return json.loads(raw)
        except ValueError as exc:
            raise ValueError(
                f"job search failed: feed file {feed_file} is not valid JSON: {exc}"
            ) from exc
    try:
        resp = requests.get(
            REMOTEOK_API_URL, headers={"User-Agent": _USER_AGENT}, timeout=15
        )
    except requests.RequestException as exc:
        raise ValueError(
            f"job search failed: could not reach the RemoteOK feed: {exc}"
        ) from exc
    if resp.status_code != 200:
        raise ValueError(
            f"job search failed: RemoteOK returned HTTP {resp.status_code}"
        )
    try:
        return resp.json()
    except ValueError as exc:
        raise ValueError(
            f"job search failed: the RemoteOK feed was not valid JSON: {exc}"
        ) from exc


def _posting(item: Dict[str, Any]) -> Dict[str, Any]:
    """Trim one raw feed entry down to the fields callers need."""
    tags = item.get("tags") or []
    if not isinstance(tags, list):
        tags = [str(tags)]
    return {
        "id": str(item.get("id", "")),
        "title": str(item.get("position", "")),
        "company": str(item.get("company", "")),
        "tags": [str(tag) for tag in tags],
        "location": str(item.get("location", "") or ""),
        "url": str(item.get("url", "") or ""),
        "date": str(item.get("date", "") or ""),
    }


def _matches(posting: Dict[str, Any], terms: List[str]) -> bool:
    """A posting matches when every keyword term shows up somewhere in
    its title, company, or tags. No terms means everything matches."""
    if not terms:
        return True
    haystack = " ".join(
        [posting["title"], posting["company"], *posting["tags"]]
    ).lower()
    return all(term in haystack for term in terms)


class JobSearchTool(Tool):
    """Search remote job postings on RemoteOK by keyword."""

    name = "job_search"
    description = (
        "Search remote job postings (RemoteOK) by keywords matched "
        "against each posting's title, company, and tags. Every word in "
        "keywords must match. Returns compact postings with title, "
        "company, tags, location, url, and date."
    )
    parameters = {
        "keywords": {
            "type": "string",
            "description": "Keywords to match, e.g. 'python django'",
            "required": True,
        },
        "limit": {
            "type": "integer",
            "description": "Max postings to return (default 10, cap 25)",
            "default": DEFAULT_LIMIT,
        },
    }

    def execute(self, keywords: str, limit: int = DEFAULT_LIMIT) -> Dict[str, Any]:
        try:
            limit = int(limit)
        except (TypeError, ValueError):
            raise ValueError(f"job search failed: limit must be a number, got {limit!r}")
        limit = max(1, min(limit, MAX_LIMIT))

        payload = _fetch_feed()
        if not isinstance(payload, list):
            raise ValueError(
                "job search failed: the RemoteOK feed had an unexpected "
                f"shape (expected a list, got {type(payload).__name__})"
            )
        items = payload
        # The first element of the feed is metadata (timestamps, legal
        # text), not a posting. It is the one entry with no position.
        if items and isinstance(items[0], dict) and "position" not in items[0]:
            items = items[1:]

        terms = [t for t in re.split(r"[\s,]+", (keywords or "").lower()) if t]
        jobs = []
        for item in items:
            if not isinstance(item, dict) or "position" not in item:
                continue
            posting = _posting(item)
            if _matches(posting, terms):
                jobs.append(posting)
        return {
            "keywords": keywords,
            "count": len(jobs[:limit]),
            "jobs": jobs[:limit],
        }


def _applications_log_path() -> Path:
    """Where application records are appended, one JSON object a line."""
    override = os.environ.get("BOTROPOLIS_APPLICATIONS_LOG")
    if override:
        return Path(override).expanduser()
    return Path(__file__).resolve().parents[1] / "data" / "applications.jsonl"


def _read_applications(path: Path) -> List[Dict[str, Any]]:
    """Read back the recorded applications. A missing log is empty."""
    if not path.exists():
        return []
    records = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            records.append(json.loads(line))
        except ValueError:
            continue
    return records


class JobApplyTool(Tool):
    """Record a prepared job application. Does not submit anything."""

    name = "job_apply"
    description = (
        "Prepare a job application: check the resume exists in the "
        "workspace and record the application (title, company, apply "
        "url, resume) in the local applications log. This does NOT "
        "submit to the employer; it returns the apply url so a human "
        "can finish the submission. Recording the same title and "
        "company twice returns a warning instead of a duplicate."
    )
    parameters = {
        "title": {
            "type": "string",
            "description": "Job title, as it appears in the posting",
            "required": True,
        },
        "company": {
            "type": "string",
            "description": "Company name, as it appears in the posting",
            "required": True,
        },
        "url": {
            "type": "string",
            "description": "The posting's apply url",
            "required": True,
        },
        "resume": {
            "type": "string",
            "description": "Resume path inside the workspace, e.g. 'resume.txt'",
            "required": True,
        },
    }

    def execute(
        self, title: str, company: str, url: str, resume: str
    ) -> Dict[str, Any]:
        resume_path = resolve_workspace_path(resume)
        if not resume_path.is_file():
            raise ValueError(f"resume not found in the workspace: {resume}")

        log_path = _applications_log_path()
        for existing in _read_applications(log_path):
            if (
                str(existing.get("company", "")).strip().lower()
                == (company or "").strip().lower()
                and str(existing.get("title", "")).strip().lower()
                == (title or "").strip().lower()
            ):
                return {
                    "status": "duplicate",
                    "warning": (
                        f"already recorded an application for {title} at "
                        f"{company} on {existing.get('timestamp', 'an earlier run')}; "
                        "nothing new was written"
                    ),
                    "application": existing,
                    "apply_url": url,
                }

        record = {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "title": title,
            "company": company,
            "url": url,
            "resume": workspace_relative(resume_path),
            "status": "prepared",
        }
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(record) + "\n")
        return {
            "status": "prepared",
            "application": record,
            "apply_url": url,
            "note": (
                "Recorded locally, nothing was submitted. "
                "Use apply_url to submit the application."
            ),
        }
