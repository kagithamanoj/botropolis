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
