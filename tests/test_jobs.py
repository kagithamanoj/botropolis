"""Job tool tests. The RemoteOK feed is stubbed with a small recorded
payload shaped like the real API response; nothing here touches the
network."""

import copy
import json

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


# --- job_apply -------------------------------------------------------


@pytest.fixture()
def job_apply(monkeypatch, tmp_path):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    (workspace / "resume.txt").write_text("Jane Doe, python engineer\n")
    monkeypatch.setenv("BOTROPOLIS_WORKSPACE", str(workspace))
    log = tmp_path / "applications.jsonl"
    monkeypatch.setenv("BOTROPOLIS_APPLICATIONS_LOG", str(log))
    return get_tool("job_apply"), log


def test_apply_records_the_application(job_apply):
    tool, log = job_apply
    out = tool.execute(
        title="Senior Python Engineer",
        company="Acme Corp",
        url="https://remoteok.com/remote-jobs/101",
        resume="resume.txt",
    )
    assert out["status"] == "prepared"
    assert out["apply_url"] == "https://remoteok.com/remote-jobs/101"

    lines = log.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) == 1
    record = json.loads(lines[0])
    assert record["title"] == "Senior Python Engineer"
    assert record["company"] == "Acme Corp"
    assert record["url"] == "https://remoteok.com/remote-jobs/101"
    assert record["resume"] == "resume.txt"
    assert record["status"] == "prepared"
    assert record["timestamp"]
    assert out["application"] == record


def test_apply_missing_resume_is_a_loud_error(job_apply):
    tool, log = job_apply
    with pytest.raises(ValueError, match="resume not found"):
        tool.execute(
            title="Senior Python Engineer",
            company="Acme Corp",
            url="https://remoteok.com/remote-jobs/101",
            resume="no_such_resume.txt",
        )
    assert not log.exists()


def test_apply_duplicate_warns_and_writes_nothing_new(job_apply):
    tool, log = job_apply
    tool.execute(
        title="Senior Python Engineer",
        company="Acme Corp",
        url="https://remoteok.com/remote-jobs/101",
        resume="resume.txt",
    )
    out = tool.execute(
        title="senior python engineer",
        company="ACME CORP",
        url="https://remoteok.com/remote-jobs/101",
        resume="resume.txt",
    )
    assert out["status"] == "duplicate"
    assert "already recorded" in out["warning"]
    lines = log.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) == 1


def test_apply_different_role_at_same_company_is_not_a_duplicate(job_apply):
    tool, log = job_apply
    tool.execute(
        title="Senior Python Engineer",
        company="Acme Corp",
        url="https://remoteok.com/remote-jobs/101",
        resume="resume.txt",
    )
    out = tool.execute(
        title="Data Engineer",
        company="Acme Corp",
        url="https://remoteok.com/remote-jobs/103",
        resume="resume.txt",
    )
    assert out["status"] == "prepared"
    lines = log.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) == 2


def test_apply_resume_outside_the_workspace_is_refused(job_apply):
    tool, log = job_apply
    with pytest.raises(ValueError, match="escapes the workspace"):
        tool.execute(
            title="Senior Python Engineer",
            company="Acme Corp",
            url="https://remoteok.com/remote-jobs/101",
            resume="../outside.txt",
        )
    assert not log.exists()
