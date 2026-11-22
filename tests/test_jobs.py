"""Job tool tests. The RemoteOK feed is stubbed with a small recorded
payload shaped like the real API response; nothing here touches the
network."""

import copy

import pytest
import requests

from botropolis.tools import jobs
from botropolis.tools import get_tool


FEED = [
    # Element zero is feed metadata, not a posting.
    {"last_updated": 1763776800, "legal": "API terms and conditions"},
    {
        "id": "101",
        "position": "Senior Python Engineer",
        "company": "Acme Corp",
        "tags": ["python", "django"],
        "location": "Worldwide",
        "url": "https://remoteok.com/remote-jobs/101",
        "date": "2026-11-20T10:00:00+00:00",
    },
    {
        "id": "102",
        "position": "Frontend Developer",
        "company": "Globex",
        "tags": ["react", "css"],
        "location": "USA only",
        "url": "https://remoteok.com/remote-jobs/102",
        "date": "2026-11-19T09:30:00+00:00",
    },
    {
        "id": "103",
        "position": "Data Engineer",
        "company": "Initech",
        "tags": ["python", "airflow"],
        "location": "Worldwide",
        "url": "https://remoteok.com/remote-jobs/103",
        "date": "2026-11-18T08:15:00+00:00",
    },
]


@pytest.fixture()
def job_search():
    return get_tool("job_search")


@pytest.fixture()
def stub_feed(monkeypatch):
    monkeypatch.setattr(jobs, "_fetch_feed", lambda: copy.deepcopy(FEED))
    return FEED


def test_search_matches_title_company_and_tags(job_search, stub_feed):
    by_tag = job_search.execute(keywords="django")
    assert [j["id"] for j in by_tag["jobs"]] == ["101"]

    by_company = job_search.execute(keywords="globex")
    assert [j["id"] for j in by_company["jobs"]] == ["102"]

    by_title = job_search.execute(keywords="frontend")
    assert [j["id"] for j in by_title["jobs"]] == ["102"]


def test_search_requires_every_keyword_term(job_search, stub_feed):
    out = job_search.execute(keywords="python airflow")
    assert [j["id"] for j in out["jobs"]] == ["103"]


def test_search_skips_the_metadata_element(job_search, stub_feed):
    # "legal" only appears in the metadata element; it must not match.
    out = job_search.execute(keywords="legal")
    assert out["jobs"] == []
    assert out["count"] == 0


def test_search_returns_compact_postings(job_search, stub_feed):
    out = job_search.execute(keywords="python")
    assert [j["id"] for j in out["jobs"]] == ["101", "103"]
    first = out["jobs"][0]
    assert first == {
        "id": "101",
        "title": "Senior Python Engineer",
        "company": "Acme Corp",
        "tags": ["python", "django"],
        "location": "Worldwide",
        "url": "https://remoteok.com/remote-jobs/101",
        "date": "2026-11-20T10:00:00+00:00",
    }


def test_search_limit_is_capped_at_25(job_search, monkeypatch):
    big_feed = [{"legal": "metadata"}] + [
        {
            "id": str(i),
            "position": "Python Developer",
            "company": f"Company {i}",
            "tags": ["python"],
            "location": "",
            "url": f"https://remoteok.com/remote-jobs/{i}",
            "date": "",
        }
        for i in range(30)
    ]
    monkeypatch.setattr(jobs, "_fetch_feed", lambda: big_feed)
    out = job_search.execute(keywords="python", limit=100)
    assert out["count"] == 25
    assert len(out["jobs"]) == 25

    out = job_search.execute(keywords="python", limit=2)
    assert [j["id"] for j in out["jobs"]] == ["0", "1"]


class _FakeResponse:
    def __init__(self, status_code=200, payload=None, json_error=False):
        self.status_code = status_code
        self._payload = payload
        self._json_error = json_error

    def json(self):
        if self._json_error:
            raise ValueError("no json here")
        return self._payload


def test_search_network_failure_is_a_loud_error(job_search, monkeypatch):
    def boom(*args, **kwargs):
        raise requests.ConnectionError("dns exploded")

    monkeypatch.setattr(jobs.requests, "get", boom)
    with pytest.raises(ValueError, match="could not reach the RemoteOK feed"):
        job_search.execute(keywords="python")


def test_search_non_200_is_a_loud_error(job_search, monkeypatch):
    monkeypatch.setattr(
        jobs.requests, "get", lambda *a, **k: _FakeResponse(status_code=503)
    )
    with pytest.raises(ValueError, match="HTTP 503"):
        job_search.execute(keywords="python")


def test_search_malformed_json_is_a_loud_error(job_search, monkeypatch):
    monkeypatch.setattr(
        jobs.requests,
        "get",
        lambda *a, **k: _FakeResponse(status_code=200, json_error=True),
    )
    with pytest.raises(ValueError, match="not valid JSON"):
        job_search.execute(keywords="python")


def test_search_wrong_shape_payload_is_a_loud_error(job_search, monkeypatch):
    monkeypatch.setattr(jobs, "_fetch_feed", lambda: {"jobs": []})
    with pytest.raises(ValueError, match="unexpected shape"):
        job_search.execute(keywords="python")
