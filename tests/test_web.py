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


def test_api_routes_still_work():
    assert client.get("/health").status_code == 200
    assert client.get("/agents").status_code == 200
    assert client.get("/departments").status_code == 200
    resp = client.post("/ask", json={"request": "ping"})
    assert resp.status_code == 200
    assert "summary" in resp.json()
