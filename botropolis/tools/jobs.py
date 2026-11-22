"""Job hunting tools backed by the RemoteOK public feed.

job_search reads the RemoteOK API (no key needed) and returns postings
that match a set of keywords. The feed's first element is metadata, not
a job, so it is skipped. A search that finds nothing is a normal empty
result, but a search that could not run is an error: network failures,
bad HTTP statuses, and malformed payloads all raise, so nobody mistakes
a broken feed for "no jobs today".

Tests and evals can point BOTROPOLIS_JOBS_FEED_FILE at a saved copy of
the feed payload to run without the network.
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any, Dict, List

import requests

from botropolis.tools.builtin import Tool

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
