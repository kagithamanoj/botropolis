"""Tests for the web UI: the index page and its static assets."""

from fastapi.testclient import TestClient

from botropolis.server import app

client = TestClient(app)


def test_index_returns_html_with_app_root():
    resp = client.get("/")
    assert resp.status_code == 200
    assert "text/html" in resp.headers["content-type"]
    assert 'id="app"' in resp.text
    assert "/static/app.js" in resp.text


def test_static_js_served():
    resp = client.get("/static/app.js")
    assert resp.status_code == 200
    assert "Botropolis web UI" in resp.text


def test_static_css_served():
    resp = client.get("/static/styles.css")
    assert resp.status_code == 200
    assert "topbar" in resp.text


def test_index_has_team_tab():
    resp = client.get("/")
    assert resp.status_code == 200
    assert 'id="tab-team"' in resp.text
    assert 'id="view-team"' in resp.text
    assert 'id="team-form"' in resp.text
    assert 'id="team-agents"' in resp.text
    assert 'id="team-rounds"' in resp.text


def test_api_routes_still_work():
    assert client.get("/health").status_code == 200
    assert client.get("/agents").status_code == 200
    assert client.get("/departments").status_code == 200
    resp = client.post("/ask", json={"request": "ping"})
    assert resp.status_code == 200
    assert "summary" in resp.json()


def test_ask_accepts_chat_history():
    resp = client.post(
        "/ask",
        json={
            "request": "And the second one?",
            "history": [
                {"role": "user", "content": "List two options."},
                {"role": "assistant", "content": "Option A and option B."},
            ],
        },
    )
    assert resp.status_code == 200
    assert "summary" in resp.json()


def test_ask_rejects_bad_history_role():
    resp = client.post(
        "/ask",
        json={
            "request": "hi",
            "history": [{"role": "system", "content": "sneaky"}],
        },
    )
    assert resp.status_code == 422


def test_ask_rejects_too_much_history():
    resp = client.post(
        "/ask",
        json={
            "request": "hi",
            "history": [{"role": "user", "content": "x"} for _ in range(25)],
        },
    )
    assert resp.status_code == 422


def test_ask_agent_accepts_history():
    resp = client.post(
        "/agents/Noah/ask",
        json={
            "request": "And the second one?",
            "history": [{"role": "user", "content": "List two options."}],
        },
    )
    assert resp.status_code == 200
    assert resp.json()["agent_name"] == "Noah"


def test_ask_agent_rejects_bad_history_role():
    resp = client.post(
        "/agents/Noah/ask",
        json={
            "request": "hi",
            "history": [{"role": "system", "content": "sneaky"}],
        },
    )
    assert resp.status_code == 422


def test_agent_task_carries_history():
    from botropolis.server import _agent_task, registry

    agent = registry.get("Noah")
    task = _agent_task(
        agent,
        "follow up",
        [{"role": "user", "content": "What is 2+2?"}],
    )
    assert task.agent_name == "Noah"
    assert "user: What is 2+2?" in task.context.get("chat_history", "")


def test_parse_history_param():
    import json

    from botropolis.server import _parse_history_param

    good = json.dumps(
        [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
        ]
    )
    assert _parse_history_param(good) == [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "hello"},
    ]
    assert _parse_history_param("not json") == []
    assert _parse_history_param("[]") == []
    # bad roles are dropped, good ones kept
    mixed = json.dumps(
        [
            {"role": "system", "content": "sneaky"},
            {"role": "user", "content": "ok"},
        ]
    )
    assert _parse_history_param(mixed) == [{"role": "user", "content": "ok"}]
    # capped at 20
    many = json.dumps([{"role": "user", "content": "x"} for _ in range(30)])
    assert len(_parse_history_param(many)) == 20


def test_stream_accepts_history_param():
    import json

    history = json.dumps([{"role": "user", "content": "What is 2+2?"}])
    with client.stream(
        "GET",
        "/agents/Noah/ask/stream",
        params={"request": "And the second one?", "history": history},
    ) as resp:
        assert resp.status_code == 200
        body = resp.read().decode()
    assert "event: done" in body


def test_stream_tolerates_bad_history_param():
    with client.stream(
        "GET",
        "/agents/Noah/ask/stream",
        params={"request": "hi", "history": "not json"},
    ) as resp:
        assert resp.status_code == 200
        body = resp.read().decode()
    assert "event: done" in body
